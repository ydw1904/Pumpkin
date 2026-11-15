use std::sync::atomic::{AtomicI32, AtomicU16, Ordering};

/// Stands for an opener or closer that isn't known, like a menu opened without a user.
const NO_USER: i32 = i32::MIN;

#[derive(Debug)]
pub struct ViewerCountTracker {
    pub old: AtomicU16,
    pub current: AtomicU16,
    /// The entity whose open or close last took the count from or to zero.
    user: AtomicI32,
}

impl Default for ViewerCountTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewerCountTracker {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            old: AtomicU16::new(0),
            current: AtomicU16::new(0),
            user: AtomicI32::new(NO_USER),
        }
    }

    pub fn open_container(&self) {
        self.open_container_by(NO_USER);
    }

    pub fn close_container(&self) {
        self.close_container_by(NO_USER);
    }

    /// Opens the container for the entity `user`, the source of the `container_open` game
    /// event when it is the first viewer.
    pub fn open_container_by(&self, user: i32) {
        if self.current.fetch_add(1, Ordering::Relaxed) == 0 {
            self.user.store(user, Ordering::Relaxed);
        }
    }

    /// Closes the container for the entity `user`, the source of the `container_close` game
    /// event when it is the last viewer.
    pub fn close_container_by(&self, user: i32) {
        if self.current.fetch_sub(1, Ordering::Relaxed) == 1 {
            self.user.store(user, Ordering::Relaxed);
        }
    }

    /// The entity id of whoever last opened the container from or closed it to no viewers.
    pub fn user(&self) -> Option<i32> {
        let user = self.user.load(Ordering::Relaxed);
        (user != NO_USER).then_some(user)
    }

    /// Returns the current number of players viewing this container
    pub fn get_viewer_count(&self) -> u16 {
        self.current.load(Ordering::Relaxed)
    }
}
