//! Optional WebGPU runtime for compiler-emitted AgentIR artifact packages.
//!
//! The runtime owns adapter discovery and device interaction, but has no
//! correctness authority and never mutates compiler IR or hash contracts.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use agentir_backend_wgsl::offline_validate_modules;
use agentir_core::{
    backend::{DeviceFingerprintHash, device_fingerprint_hash},
    backend_ir::{
        ArtifactBindingLayout, ArtifactPackage, BackendBindingAccess, BackendExtent,
        BackendParameterType, DeviceFingerprint,
    },
    diagnostics::{AgentError, AgentResult, ErrorCode},
    target::{TargetManifest, WEBGPU_WGSL_V1},
    types::{DimExpr, ScalarType},
};
use std::collections::BTreeMap;
use wgpu::util::DeviceExt;

/// Runtime implementation version included in device fingerprints.
pub const WGPU_RUNTIME_VERSION: &str = "agentir-runtime-wgpu-v1/wgpu-24";

/// One discovered adapter and its separate fingerprint hash.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeviceRecord {
    /// Stable zero-based adapter selector for the current discovery call.
    pub index: u32,
    /// Reported device/runtime fingerprint.
    pub fingerprint: DeviceFingerprint,
    /// Exact fingerprint hash, excluded from compiler correctness identities.
    pub fingerprint_hash: DeviceFingerprintHash,
    /// Whether reported limits satisfy the selected target.
    pub target_compatible: bool,
}

/// Structured device execution input.
#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeInput {
    /// Scalar f32 uniform input.
    F32(f32),
    /// Scalar i32 uniform input.
    I32(i32),
    /// One-dimensional f32 tensor storage input.
    F32Tensor(Vec<f32>),
}

/// Device execution result and confidence-only trace metadata.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeviceExecutionRecord {
    /// Fingerprint of the executing adapter.
    pub device: DeviceFingerprint,
    /// Fingerprint hash.
    pub device_fingerprint_hash: DeviceFingerprintHash,
    /// Exact artifact hash executed.
    pub artifact_hash: agentir_core::backend::ArtifactHash,
    /// External f32 tensor outputs.
    pub outputs: BTreeMap<String, Vec<f32>>,
    /// Selected guarded branch, when the artifact is guarded.
    pub guard_branch: Option<bool>,
    /// Ordered dispatch count.
    pub dispatch_count: usize,
}

fn runtime_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

fn backend_name(backend: wgpu::Backend) -> String {
    format!("{backend:?}").to_lowercase()
}

fn fingerprint(adapter: &wgpu::Adapter) -> DeviceFingerprint {
    let info = adapter.get_info();
    let limits = adapter.limits();
    let mut reported = BTreeMap::new();
    reported.insert(
        "max_compute_invocations_per_workgroup".to_owned(),
        u64::from(limits.max_compute_invocations_per_workgroup),
    );
    reported.insert(
        "max_compute_workgroup_size_x".to_owned(),
        u64::from(limits.max_compute_workgroup_size_x),
    );
    reported.insert(
        "max_compute_workgroup_size_y".to_owned(),
        u64::from(limits.max_compute_workgroup_size_y),
    );
    reported.insert(
        "max_compute_workgroup_size_z".to_owned(),
        u64::from(limits.max_compute_workgroup_size_z),
    );
    reported.insert(
        "max_compute_workgroups_per_dimension".to_owned(),
        u64::from(limits.max_compute_workgroups_per_dimension),
    );
    reported.insert(
        "max_storage_buffer_binding_size".to_owned(),
        u64::from(limits.max_storage_buffer_binding_size),
    );
    DeviceFingerprint {
        backend_api: backend_name(info.backend),
        adapter_name: info.name,
        vendor_id: Some(info.vendor),
        device_id: Some(info.device),
        driver_info: Some(format!("{} / {}", info.driver, info.driver_info)),
        limits: reported,
        runtime_version: WGPU_RUNTIME_VERSION.to_owned(),
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

fn compatible(fingerprint: &DeviceFingerprint, target: &TargetManifest) -> bool {
    if target.profile != WEBGPU_WGSL_V1 {
        return false;
    }
    let limit = |name: &str| fingerprint.limits.get(name).copied().unwrap_or(0);
    limit("max_compute_invocations_per_workgroup") >= target.hierarchy.max_threads_per_workgroup
        && limit("max_compute_workgroup_size_x") >= target.hierarchy.max_workgroup_dimensions[0]
        && limit("max_compute_workgroup_size_y") >= target.hierarchy.max_workgroup_dimensions[1]
        && limit("max_compute_workgroup_size_z") >= target.hierarchy.max_workgroup_dimensions[2]
        && limit("max_compute_workgroups_per_dimension") >= target.hierarchy.max_grid_dimensions[0]
}

fn adapters() -> Vec<wgpu::Adapter> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    instance.enumerate_adapters(wgpu::Backends::all())
}

/// Discovers adapters without mutating compiler state.
pub fn list_devices(target: &TargetManifest) -> AgentResult<Vec<DeviceRecord>> {
    let mut records = Vec::new();
    for (index, adapter) in adapters().into_iter().enumerate() {
        let fingerprint = fingerprint(&adapter);
        records.push(DeviceRecord {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            fingerprint_hash: device_fingerprint_hash(&fingerprint)?,
            target_compatible: compatible(&fingerprint, target),
            fingerprint,
        });
    }
    Ok(records)
}

fn extent_value(
    extent: &BackendExtent,
    inputs: &BTreeMap<String, RuntimeInput>,
) -> AgentResult<u64> {
    match extent {
        BackendExtent::Static { value } => Ok(*value),
        BackendExtent::Symbol { name } => inputs
            .values()
            .find_map(|value| match value {
                RuntimeInput::F32Tensor(values) => u64::try_from(values.len()).ok(),
                RuntimeInput::F32(_) | RuntimeInput::I32(_) => None,
            })
            .ok_or_else(|| {
                runtime_error(
                    ErrorCode::EvaluationInputMismatch,
                    format!("symbolic extent `{name}` has no tensor input"),
                )
            }),
    }
}

fn shape_elements(
    layout: &ArtifactBindingLayout,
    inputs: &BTreeMap<String, RuntimeInput>,
) -> AgentResult<u64> {
    extent_value(&layout.logical_extent, inputs)
}

fn scalar_bytes(value: &RuntimeInput, ty: BackendParameterType) -> AgentResult<[u8; 4]> {
    match (value, ty) {
        (RuntimeInput::F32(value), BackendParameterType::F32) => Ok(value.to_le_bytes()),
        (RuntimeInput::I32(value), BackendParameterType::I32) => Ok(value.to_le_bytes()),
        _ => Err(runtime_error(
            ErrorCode::EvaluationInputMismatch,
            "runtime scalar input does not match the artifact parameter ABI",
        )),
    }
}

fn parameter_bytes(
    layout: &ArtifactBindingLayout,
    inputs: &BTreeMap<String, RuntimeInput>,
) -> AgentResult<Vec<u8>> {
    let mut bytes = vec![
        0;
        usize::try_from(layout.parameter_block.byte_size).map_err(|_| {
            runtime_error(
                ErrorCode::ResourceLimitExceeded,
                "uniform block is too large",
            )
        })?
    ];
    for entry in &layout.parameter_block.entries {
        let encoded = if entry.ty == BackendParameterType::U32 {
            u32::try_from(extent_value(
                &BackendExtent::Symbol {
                    name: entry.name.clone(),
                },
                inputs,
            )?)
            .map_err(|_| runtime_error(ErrorCode::ResourceLimitExceeded, "extent exceeds u32"))?
            .to_le_bytes()
        } else {
            let value = inputs.get(&entry.name).ok_or_else(|| {
                runtime_error(
                    ErrorCode::EvaluationInputMismatch,
                    format!("missing scalar input `{}`", entry.name),
                )
            })?;
            scalar_bytes(value, entry.ty)?
        };
        let start = usize::try_from(entry.offset).map_err(|_| {
            runtime_error(
                ErrorCode::ResourceLimitExceeded,
                "uniform offset exceeds usize",
            )
        })?;
        let end = start.saturating_add(encoded.len());
        if end > bytes.len() {
            return Err(runtime_error(
                ErrorCode::ArtifactManifestInvalid,
                "uniform entry exceeds its declared parameter block",
            ));
        }
        bytes[start..end].copy_from_slice(&encoded);
    }
    Ok(bytes)
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Executes a compiler-emitted package on one compatible adapter.
///
/// Device execution is confidence evidence only. The package is offline
/// validated again before adapter/device allocation.
pub fn execute(
    package: &ArtifactPackage,
    target: &TargetManifest,
    adapter_index: u32,
    inputs: &BTreeMap<String, RuntimeInput>,
) -> AgentResult<DeviceExecutionRecord> {
    offline_validate_modules(&package.modules)?;
    if adapter_index == u32::MAX {
        return Err(runtime_error(
            ErrorCode::DeviceUnavailable,
            "WebGPU adapter is unavailable",
        ));
    }
    pollster::block_on(execute_async(package, target, adapter_index, inputs))
}

async fn execute_async(
    package: &ArtifactPackage,
    target: &TargetManifest,
    adapter_index: u32,
    inputs: &BTreeMap<String, RuntimeInput>,
) -> AgentResult<DeviceExecutionRecord> {
    let adapter = adapters()
        .into_iter()
        .nth(usize::try_from(adapter_index).unwrap_or(usize::MAX))
        .ok_or_else(|| {
            runtime_error(
                ErrorCode::DeviceUnavailable,
                "WebGPU adapter is unavailable",
            )
        })?;
    let fingerprint = fingerprint(&adapter);
    if !compatible(&fingerprint, target) {
        return Err(runtime_error(
            ErrorCode::DeviceCapabilityUnsupported,
            "adapter limits do not satisfy webgpu_wgsl_v1 TargetManifest",
        ));
    }
    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                label: Some("agentir-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        )
        .await
        .map_err(|error| {
            runtime_error(
                ErrorCode::DeviceUnavailable,
                format!("WebGPU device request failed: {error}"),
            )
        })?;

    let mut buffers = BTreeMap::<agentir_core::ids::BufferId, wgpu::Buffer>::new();
    let mut buffer_sizes = BTreeMap::<agentir_core::ids::BufferId, u64>::new();
    for layout in &package.manifest.binding_layouts {
        let logical = shape_elements(layout, inputs)?;
        for binding in &layout.storage_bindings {
            if binding.element_type != ScalarType::F32 || binding.shape.0.len() != 1 {
                return Err(runtime_error(
                    ErrorCode::ArtifactManifestInvalid,
                    "wgpu runtime v1 accepts only one-dimensional f32 storage bindings",
                ));
            }
            match &binding.shape.0[0] {
                DimExpr::Static(value) if *value != logical => {
                    return Err(runtime_error(
                        ErrorCode::ArtifactManifestInvalid,
                        "static binding shape differs from the kernel logical extent",
                    ));
                }
                DimExpr::Static(_) | DimExpr::Symbol(_) => {}
                DimExpr::Affine { .. } => {
                    return Err(runtime_error(
                        ErrorCode::ArtifactManifestInvalid,
                        "affine runtime binding shapes are unsupported by WGSL v1",
                    ));
                }
            }
            let elements = binding.offset_elements.saturating_add(logical);
            let size = elements.saturating_mul(4).max(4);
            if let Some(previous) = buffer_sizes.insert(binding.buffer.clone(), size) {
                if previous != size {
                    return Err(runtime_error(
                        ErrorCode::ArtifactManifestInvalid,
                        "one buffer has inconsistent kernel ABI sizes",
                    ));
                }
            }
            if buffers.contains_key(&binding.buffer) {
                continue;
            }
            let mut initial = vec![
                0;
                usize::try_from(size).map_err(|_| {
                    runtime_error(
                        ErrorCode::ResourceLimitExceeded,
                        "storage buffer is too large",
                    )
                })?
            ];
            if let Some(name) = &binding.external_name {
                if let Some(RuntimeInput::F32Tensor(values)) = inputs.get(name) {
                    if u64::try_from(values.len()).unwrap_or(u64::MAX) != logical {
                        return Err(runtime_error(
                            ErrorCode::EvaluationInputMismatch,
                            format!("tensor input `{name}` has the wrong length"),
                        ));
                    }
                    let start = usize::try_from(binding.offset_elements.saturating_mul(4))
                        .map_err(|_| {
                            runtime_error(
                                ErrorCode::ResourceLimitExceeded,
                                "buffer offset is too large",
                            )
                        })?;
                    let encoded = f32_bytes(values);
                    initial[start..start + encoded.len()].copy_from_slice(&encoded);
                }
            }
            buffers.insert(
                binding.buffer.clone(),
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: binding.external_name.as_deref(),
                    contents: &initial,
                    usage: wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_SRC
                        | wgpu::BufferUsages::COPY_DST,
                }),
            );
        }
    }

    let guard_branch = package
        .manifest
        .guard
        .as_ref()
        .map(|guard| match &guard.predicate {
            agentir_core::backend_ir::BackendGuardPredicate::NoOverlap {
                first, second, ..
            } => first != second,
        });
    let selected_orders = package.manifest.guard.as_ref().map(|guard| {
        if guard_branch == Some(true) {
            &guard.true_dispatches
        } else {
            &guard.false_dispatches
        }
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("agentir-command-encoder"),
    });
    let mut dispatch_count = 0usize;
    for dispatch in &package.manifest.dispatches {
        if selected_orders.is_some_and(|orders| !orders.contains(&dispatch.order)) {
            continue;
        }
        let entry = package
            .manifest
            .entry_points
            .iter()
            .find(|entry| entry.kernel == dispatch.kernel)
            .ok_or_else(|| {
                runtime_error(
                    ErrorCode::ArtifactManifestInvalid,
                    "dispatch entry point is missing",
                )
            })?;
        let layout = package
            .manifest
            .binding_layouts
            .iter()
            .find(|layout| layout.kernel == dispatch.kernel)
            .ok_or_else(|| {
                runtime_error(
                    ErrorCode::ArtifactManifestInvalid,
                    "dispatch binding ABI is missing",
                )
            })?;
        let module = package
            .modules
            .iter()
            .find(|module| module.id == entry.module)
            .ok_or_else(|| {
                runtime_error(
                    ErrorCode::ArtifactManifestInvalid,
                    "entry point module is missing",
                )
            })?;
        let mut layout_entries = layout
            .storage_bindings
            .iter()
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding: binding.binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: binding.access == BackendBindingAccess::Read,
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect::<Vec<_>>();
        if !layout.parameter_block.entries.is_empty() {
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: layout.parameter_block.binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
        }
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("agentir-bind-group-layout"),
            entries: &layout_entries,
        });
        let uniform = if layout.parameter_block.entries.is_empty() {
            None
        } else {
            Some(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("agentir-parameters"),
                    contents: &parameter_bytes(layout, inputs)?,
                    usage: wgpu::BufferUsages::UNIFORM,
                }),
            )
        };
        let mut bind_entries = layout
            .storage_bindings
            .iter()
            .map(|binding| wgpu::BindGroupEntry {
                binding: binding.binding,
                resource: buffers[&binding.buffer].as_entire_binding(),
            })
            .collect::<Vec<_>>();
        if let Some(uniform) = &uniform {
            bind_entries.push(wgpu::BindGroupEntry {
                binding: layout.parameter_block.binding,
                resource: uniform.as_entire_binding(),
            });
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("agentir-bind-group"),
            layout: &bind_group_layout,
            entries: &bind_entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("agentir-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&module.name),
            source: wgpu::ShaderSource::Wgsl(module.wgsl.clone().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(&entry.name),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some(&entry.name),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let workgroups = [
            match &dispatch.workgroups[0] {
                BackendExtent::Static { value } => *value,
                BackendExtent::Symbol { .. } => {
                    extent_value(&layout.logical_extent, inputs)?
                        .saturating_add(u64::from(dispatch.workgroup_size[0]).saturating_sub(1))
                        / u64::from(dispatch.workgroup_size[0])
                }
            },
            extent_value(&dispatch.workgroups[1], inputs)?,
            extent_value(&dispatch.workgroups[2], inputs)?,
        ];
        let workgroups = workgroups.map(|value| {
            u32::try_from(value).map_err(|_| {
                runtime_error(
                    ErrorCode::ResourceLimitExceeded,
                    "dispatch dimension exceeds u32",
                )
            })
        });
        let [x, y, z] = [
            workgroups[0].clone()?,
            workgroups[1].clone()?,
            workgroups[2].clone()?,
        ];
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("agentir-compute-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(x, y, z);
        }
        dispatch_count = dispatch_count.saturating_add(1);
    }

    let mut readbacks = Vec::new();
    for output in &package.manifest.outputs {
        let size = buffer_sizes.get(&output.buffer).copied().ok_or_else(|| {
            runtime_error(
                ErrorCode::ArtifactManifestInvalid,
                "output buffer is missing",
            )
        })?;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&output.name),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&buffers[&output.buffer], 0, &staging, 0, size);
        readbacks.push((output.clone(), staging));
    }
    queue.submit(Some(encoder.finish()));
    let mut outputs = BTreeMap::new();
    for (output, staging) in readbacks {
        let slice = staging.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        receiver
            .recv()
            .map_err(|_| {
                runtime_error(
                    ErrorCode::DeviceExecutionFailed,
                    "device map callback was dropped",
                )
            })?
            .map_err(|error| {
                runtime_error(
                    ErrorCode::DeviceExecutionFailed,
                    format!("output map failed: {error}"),
                )
            })?;
        let layout = package
            .manifest
            .binding_layouts
            .iter()
            .find(|layout| layout.outputs.contains(&output))
            .ok_or_else(|| {
                runtime_error(ErrorCode::ArtifactManifestInvalid, "output ABI is missing")
            })?;
        let binding = layout
            .storage_bindings
            .iter()
            .find(|binding| binding.buffer == output.buffer)
            .ok_or_else(|| {
                runtime_error(
                    ErrorCode::ArtifactManifestInvalid,
                    "output binding is missing",
                )
            })?;
        let logical = usize::try_from(shape_elements(layout, inputs)?).map_err(|_| {
            runtime_error(
                ErrorCode::ResourceLimitExceeded,
                "output extent exceeds usize",
            )
        })?;
        let start = usize::try_from(binding.offset_elements.saturating_mul(4)).map_err(|_| {
            runtime_error(
                ErrorCode::ResourceLimitExceeded,
                "output offset exceeds usize",
            )
        })?;
        let data = slice.get_mapped_range();
        let mut values = Vec::with_capacity(logical);
        for chunk in data[start..start + logical.saturating_mul(4)].chunks_exact(4) {
            values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        drop(data);
        staging.unmap();
        outputs.insert(output.name, values);
    }
    Ok(DeviceExecutionRecord {
        device_fingerprint_hash: device_fingerprint_hash(&fingerprint)?,
        device: fingerprint,
        artifact_hash: package.artifact_hash.clone(),
        outputs,
        guard_branch,
        dispatch_count,
    })
}
