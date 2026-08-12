#![forbid(unsafe_code)]

fn main() {
    loop {
        std::hint::spin_loop();
    }
}
