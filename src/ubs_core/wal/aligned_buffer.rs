//! Aligned buffer for O_DIRECT writes
//!
//! O_DIRECT bypasses the OS page cache for durability guarantees.
//! Requires 4KB (or 512 byte) aligned buffers.

use std::alloc::{alloc, dealloc, Layout};
use std::ptr::NonNull;

/// Default alignment for O_DIRECT (4KB for most filesystems)
pub const DEFAULT_ALIGNMENT: usize = 4096;

/// Aligned buffer for O_DIRECT writes
pub struct AlignedBuffer {
    ptr: NonNull<u8>,
    capacity: usize,
    len: usize,
    alignment: usize,
}

impl AlignedBuffer {
    /// Create a new aligned buffer
    pub fn new(capacity: usize, alignment: usize) -> Self {
        assert!(alignment.is_power_of_two(), "Alignment must be power of 2");
        assert!(capacity > 0, "Capacity must be > 0");

        let layout = Layout::from_size_align(capacity, alignment)
            .expect("Invalid layout");

        let ptr = unsafe {
            let ptr = alloc(layout);
            if ptr.is_null() {
                std::alloc::handle_alloc_error(layout);
            }
            NonNull::new_unchecked(ptr)
        };

        Self {
            ptr,
            capacity,
            len: 0,
            alignment,
        }
    }

    /// Create with default 4KB alignment
    pub fn with_capacity(capacity: usize) -> Self {
        Self::new(capacity, DEFAULT_ALIGNMENT)
    }

    /// Get current length
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Get capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Get remaining space
    pub fn remaining(&self) -> usize {
        self.capacity - self.len
    }

    /// Get alignment
    pub fn alignment(&self) -> usize {
        self.alignment
    }

    /// Write data to buffer
    /// Returns true if successful, false if not enough space
    pub fn write(&mut self, data: &[u8]) -> bool {
        if data.len() > self.remaining() {
            return false;
        }

        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.ptr.as_ptr().add(self.len),
                data.len(),
            );
        }
        self.len += data.len();
        true
    }

    /// Get buffer as slice
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    /// Get buffer as mutable slice (full capacity)
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.capacity) }
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Set length (after external writes)
    /// # Safety
    /// Caller must ensure len is valid
    pub unsafe fn set_len(&mut self, len: usize) {
        assert!(len <= self.capacity);
        self.len = len;
    }

    /// Get raw pointer (for O_DIRECT writes)
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    /// Pad to alignment boundary with zeros
    pub fn pad_to_alignment(&mut self) {
        let padding = (self.alignment - (self.len % self.alignment)) % self.alignment;
        if padding > 0 && self.remaining() >= padding {
            unsafe {
                std::ptr::write_bytes(self.ptr.as_ptr().add(self.len), 0, padding);
            }
            self.len += padding;
        }
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.capacity, self.alignment)
            .expect("Invalid layout");
        unsafe {
            dealloc(self.ptr.as_ptr(), layout);
        }
    }
}

// Safety: AlignedBuffer owns its data and doesn't share references
unsafe impl Send for AlignedBuffer {}
unsafe impl Sync for AlignedBuffer {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aligned_buffer_creation() {
        let buf = AlignedBuffer::with_capacity(8192);
        assert_eq!(buf.capacity(), 8192);
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
        assert_eq!(buf.alignment(), DEFAULT_ALIGNMENT);
    }

    #[test]
    fn test_aligned_buffer_write() {
        let mut buf = AlignedBuffer::with_capacity(4096);
        let data = b"Hello, WAL!";
        assert!(buf.write(data));
        assert_eq!(buf.len(), data.len());
        assert_eq!(buf.as_slice(), data);
    }

    #[test]
    fn test_aligned_buffer_overflow() {
        let mut buf = AlignedBuffer::with_capacity(10);
        let data = b"This is too long";
        assert!(!buf.write(data));
        assert_eq!(buf.len(), 0);  // Nothing written
    }

    #[test]
    fn test_aligned_buffer_clear() {
        let mut buf = AlignedBuffer::with_capacity(4096);
        buf.write(b"Some data");
        assert!(!buf.is_empty());
        buf.clear();
        assert!(buf.is_empty());
    }

    #[test]
    fn test_aligned_buffer_pad() {
        let mut buf = AlignedBuffer::new(8192, 512);
        buf.write(b"Hello");  // 5 bytes
        buf.pad_to_alignment();
        assert_eq!(buf.len() % 512, 0);
        assert_eq!(buf.len(), 512);
    }

    #[test]
    fn test_alignment_check() {
        let buf = AlignedBuffer::new(4096, 4096);
        assert_eq!(buf.as_ptr() as usize % 4096, 0);  // Pointer is aligned
    }
}
