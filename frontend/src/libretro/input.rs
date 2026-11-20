use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

/// Number of controller ports exposed to the core. TsundereStation currently
/// maps one SDL controller per port.
pub const PORTS: usize = 2;

/// Snapshot of one controller in libretro's RetroPad numbering. The UI thread
/// writes it; the core's input callback reads it.
struct PortInput {
    buttons: AtomicU32,
    axes: [AtomicI32; 4],
}

impl PortInput {
    fn new() -> Self {
        Self {
            buttons: AtomicU32::new(0),
            axes: [
                AtomicI32::new(0),
                AtomicI32::new(0),
                AtomicI32::new(0),
                AtomicI32::new(0),
            ],
        }
    }

    fn set(&self, buttons: u32, axes: [i32; 4]) {
        self.buttons.store(buttons, Ordering::Relaxed);
        for (dst, value) in self.axes.iter().zip(axes.iter()) {
            dst.store(*value, Ordering::Relaxed);
        }
    }

    fn clear(&self) {
        self.set(0, [0; 4]);
    }
}

pub struct InputShared {
    ports: [PortInput; PORTS],
}

impl InputShared {
    pub fn new() -> Self {
        Self {
            ports: std::array::from_fn(|_| PortInput::new()),
        }
    }

    pub fn set(&self, port: usize, buttons: u32, axes: [i32; 4]) {
        if let Some(input) = self.ports.get(port) {
            input.set(buttons, axes);
        }
    }

    pub fn clear(&self) {
        for port in &self.ports {
            port.clear();
        }
    }

    pub fn buttons(&self, port: usize) -> u32 {
        self.ports
            .get(port)
            .map(|input| input.buttons.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    pub fn axis(&self, port: usize, index: usize, id: usize) -> i16 {
        if index < 2 && id < 2 {
            let Some(input) = self.ports.get(port) else {
                return 0;
            };
            let v = input.axes[index * 2 + id].load(Ordering::Relaxed);
            v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_isolated() {
        let input = InputShared::new();
        input.set(0, 0b101, [1, 2, 3, 4]);
        input.set(1, 0b010, [-1, -2, -3, -4]);

        assert_eq!(input.buttons(0), 0b101);
        assert_eq!(input.buttons(1), 0b010);
        assert_eq!(input.axis(0, 0, 0), 1);
        assert_eq!(input.axis(0, 1, 1), 4);
        assert_eq!(input.axis(1, 0, 0), -1);
        assert_eq!(input.axis(1, 1, 1), -4);
        assert_eq!(input.buttons(2), 0);
        assert_eq!(input.axis(2, 0, 0), 0);
    }

    #[test]
    fn clear_zeroes_every_port() {
        let input = InputShared::new();
        input.set(0, 0xffff, [1, 2, 3, 4]);
        input.set(1, 0xffff, [-1, -2, -3, -4]);
        input.clear();

        for port in 0..PORTS {
            assert_eq!(input.buttons(port), 0);
            for index in 0..2 {
                for id in 0..2 {
                    assert_eq!(input.axis(port, index, id), 0);
                }
            }
        }
    }
}
