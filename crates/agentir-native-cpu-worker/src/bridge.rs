//! Sole AgentIR-owned unsafe boundary for calling finalized worker-local code.

use crate::{NATIVE_CALL_ABI_VERSION, NativeWorkerError, NativeWorkerResult};
use std::{mem, ptr::NonNull};

pub(crate) fn invoke(
    code: NonNull<u8>,
    packed: &mut [f32],
    expected_values: usize,
    code_alignment: usize,
    abi_version: u32,
) -> NativeWorkerResult<()> {
    if abi_version != NATIVE_CALL_ABI_VERSION {
        return Err(NativeWorkerError::new(
            "NATIVE_ABI_VERSION",
            "native call ABI version is unsupported",
        ));
    }
    if packed.len() != expected_values || packed.len() > isize::MAX as usize / mem::size_of::<f32>()
    {
        return Err(NativeWorkerError::new(
            "NATIVE_ABI_LENGTH",
            "packed buffer length is invalid for the native ABI",
        ));
    }
    if code_alignment == 0
        || !code_alignment.is_power_of_two()
        || code.as_ptr().align_offset(code_alignment) != 0
    {
        return Err(NativeWorkerError::new(
            "NATIVE_ENTRY_ALIGNMENT",
            "finalized entry point does not satisfy the target function alignment",
        ));
    }
    let data = packed.as_mut_ptr();
    if data.is_null() || data.align_offset(mem::align_of::<f32>()) != 0 {
        return Err(NativeWorkerError::new(
            "NATIVE_ABI_ALIGNMENT",
            "packed buffer pointer is null or misaligned",
        ));
    }

    // SAFETY: `code` is the non-null finalized entry point of the only locally
    // declared, target-aligned Cranelift function, whose verified signature is exactly
    // `unsafe extern "C" fn(*mut f32)`. `data` is aligned and points to the
    // checked `expected_values` writable elements in `packed`; the allocation
    // remains alive and exclusively borrowed until this single call returns.
    unsafe {
        let function = mem::transmute::<NonNull<u8>, unsafe extern "C" fn(*mut f32)>(code);
        function(data);
    }
    Ok(())
}
