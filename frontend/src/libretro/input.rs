use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

/// Snapshot of the currently selected SDL controller, in libretro's RetroPad
/// numbering. The UI thread writes it; the core's input callback reads it.
pub struct InputShared {
    buttons: AtomicU32,
    axes: [AtomicI32; 4],
}

impl InputShared {
    pub fn new() -> Self {
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

    pub fn set(&self, buttons: u32, axes: [i32; 4]) {
        self.buttons.store(buttons, Ordering::Relaxed);
        for (dst, value) in self.axes.iter().zip(axes.iter()) {
            dst.store(*value, Ordering::Relaxed);
        }
    }

    pub fn buttons(&self) -> u32 {
        self.buttons.load(Ordering::Relaxed)
    }

    pub fn axis(&self, index: usize, id: usize) -> i16 {
        if index < 2 && id < 2 {
            let v = self.axes[index * 2 + id].load(Ordering::Relaxed);
            v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
        } else {
            0
        }
    }
}
