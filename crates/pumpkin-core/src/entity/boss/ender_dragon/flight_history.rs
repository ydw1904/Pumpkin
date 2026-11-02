#[derive(Clone, Copy, Default, Debug)]
pub struct Sample {
    pub y: f64,
    pub y_rot: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct DragonFlightHistory {
    samples: [Sample; 64],
    head: i32,
}

impl Default for DragonFlightHistory {
    fn default() -> Self {
        Self {
            samples: [Sample::default(); 64],
            head: -1,
        }
    }
}

impl DragonFlightHistory {
    /// Vanilla `DragonFlightHistory.record`: the first sample fills the whole history, so the
    /// tail doesn't start out stretched down to y 0.
    pub const fn record(&mut self, y: f64, y_rot: f32) {
        let sample = Sample { y, y_rot };
        if self.head < 0 {
            self.samples = [sample; 64];
        }
        self.head += 1;
        if self.head >= 64 {
            self.head = 0;
        }
        self.samples[self.head as usize] = sample;
    }

    #[must_use]
    pub fn get(&self, offset: i32) -> Sample {
        if self.head < 0 {
            return Sample::default();
        }
        let mut index = (self.head - offset) & 63;
        if index < 0 {
            index += 64;
        }
        self.samples[index as usize]
    }
}
