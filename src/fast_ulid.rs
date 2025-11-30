use ulid::Ulid;

// ==========================================
// 1. FAST ULID GENERATOR
// ==========================================

pub struct FastUlidGenerator {
    generator: ulid::Generator,
}

impl FastUlidGenerator {
    pub fn new() -> Self {
        Self {
            generator: ulid::Generator::new(),
        }
    }

    #[inline(always)]
    pub fn generate(&mut self) -> Ulid {
        // This uses Monotonic logic internally:
        // If time hasn't changed, it increments the random part.
        // It avoids the syscall overhead 99.9% of the time.
        self.generator.generate().unwrap()
    }
}
