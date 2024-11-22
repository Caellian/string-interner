#![allow(dead_code)]

use super::InternedStr;
use core::{fmt::Debug, marker::PhantomPinned, pin::Pin};

/// Open bucket is a wrapper for mutable sequence of bytes.
///
/// A bucket is: contiguous, uniquely owned, and [pinned].
///
/// Bucket behaves much like [`String`], but it can't be extended and thanks to that
/// restriction it guarantees the underlying data will never be moved after it has been
/// allocated.
/// 
/// An open bucket may be closed and turned into a [`ClosedBucket`] using [`Into`].
///
/// [pinned]: core::pin
/// [`String`]: alloc::string::String
#[derive(Debug, PartialEq, Eq)]
#[repr(C)]
pub struct OpenBucket<'i> {
    data: Pin<&'i mut [u8]>,
    len: usize,
    _pinned: PhantomPinned,
}

impl<'i> OpenBucket<'i> {
    /// Creates a new fixed string with the given fixed `capacity`.
    pub fn with_capacity(capacity: usize) -> Self {
        if capacity > isize::MAX as usize {
            panic!("max addressable allocation size exceeded: {}", capacity)
        }
        let buffer = unsafe {
            // SAFETY: size constraints validated for `capacity` above; `u8` array can be
            //         allocated with alignment of 1; any size is multiple of 1.
            let layout = core::alloc::Layout::from_size_align_unchecked(capacity, 1);
            let buffer = alloc::alloc::alloc(layout);
            // SAFETY: slice was allocated with Layout of `capacity` size
            core::slice::from_raw_parts_mut(buffer, capacity)
        };
        Self {
            data: Pin::new(buffer),
            len: 0,
            _pinned: PhantomPinned,
        }
    }

    /// Returns the total capacity of the fixed string, in bytes.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    /// Returns the length of the fixed string, in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns a pointer to bucket data.
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }

    /// Returns a pointer range of bucket data.
    ///
    /// This range can be assumed to contain only UTF-8 characters.
    #[inline]
    pub fn as_ptr_range(&self) -> core::ops::Range<*const u8> {
        unsafe { self.as_ptr()..(self.as_ptr().add(self.len())) }
    }

    /// Returns a pinned `&'i str` reference to owned data.
    #[inline]
    pub fn as_str(&'i self) -> Pin<&'i str> {
        unsafe {
            Pin::map_unchecked(self.data.as_ref(), |data: &[u8]| {
                // SAFETY: filled sections must be UTF-8 because data was copied from `str`
                std::str::from_utf8_unchecked(&data[..self.len])
            })
            // NOTE: can't extend borrow duration because open bucket is mutable
        }
    }

    /// Returns a pinned `&'i mut str` reference to owned data.
    #[inline]
    pub fn as_str_mut(&'i mut self) -> Pin<&'i mut str> {
        unsafe {
            Pin::map_unchecked_mut(self.data.as_mut(), |data: &mut [u8]| {
                // SAFETY: filled sections must be UTF-8 because data was copied from `str`
                std::str::from_utf8_unchecked_mut(&mut data[..self.len])
            })
        }
    }

    /// Returns `true` if the bucket can store `additional` bytes.
    #[inline]
    pub fn can_store(&self, additional: usize) -> bool {
        !self.data.is_empty() && self.capacity() - self.len >= additional
    }

    /// Pushes the given string into the fixed string if there is enough capacity.
    ///
    /// Returns an [`InternedStr<'i>`] if there was enough free space left, or
    /// [`ExceedsCapacityError`] otherwise.
    pub fn push_str(&mut self, string: &str) -> Result<InternedStr<'i>, ExceedsCapacityError> {
        if self.capacity() - self.len < string.len() {
            return Err(ExceedsCapacityError {
                requested: string.len(),
                remaining: self.capacity() - self.len,
            });
        }

        Ok(unsafe {
            //SAFETY: Checked whether `string` fits in the bucket above.
            self.push_str_unchecked(string)
        })
    }

    /// Pushes the given `string` into the bucket, without checking whether there's enough
    /// space left.
    ///
    /// # Safety
    ///
    /// This function is safe if the bucket is known to have enough space to store
    /// additional `string.len()` bytes.
    pub(super) unsafe fn push_str_unchecked(&mut self, string: &str) -> InternedStr<'i> {
        let start_len = self.len;
        unsafe {
            self.extend_from_slice_unchecked(string.as_bytes());
        }
        // Now [start_len, self.len> range is the pushed string.

        let interned = {
            let data = unsafe {
                // SAFETY: extend_from_slice_unchecked above copied `self.len - start_len` bytes to `start_len` location.
                core::slice::from_raw_parts(self.data.as_ptr().add(start_len), self.len - start_len)
            };
            let data = unsafe {
                // SAFETY: Interned bytes will be valid for the duration of container,
                //         i.e. until end of 'i.
                std::mem::transmute::<&[u8], &'i [u8]>(data)
            };
            Pin::new(unsafe {
                // SAFETY:
                // - Input string was UTF-8, so a verbatim copy of its bytes will be
                //   as well.
                // - `self.len` was moved to the end of this string above, so it
                //   won't be invalidated during use.
                core::str::from_utf8_unchecked(data)
            })
        };

        InternedStr::new(interned)
    }

    /// Extends the bucket with provided `data`, and updates the end marker.
    ///
    /// Returns remaining free space after extension, or [`ExceedsCapacityError`] if there
    /// wasn't enough space to append all bytes from data.
    pub fn extend_from_slice(
        &mut self,
        data: impl AsRef<[u8]>,
    ) -> Result<usize, ExceedsCapacityError> {
        if self.capacity() - self.len < data.as_ref().len() {
            return Err(ExceedsCapacityError {
                requested: data.as_ref().len(),
                remaining: self.capacity() - self.len,
            });
        }

        Ok(unsafe {
            //SAFETY: Checked whether `data` fits in the bucket above.
            self.extend_from_slice_unchecked(data)
        })
    }

    /// Extends the bucket with provided `data`, and updates the end marker.
    ///
    /// Returns remaining free space after extension.
    ///
    /// # Safety
    ///
    /// This function is safe if the bucket is known to have enough space to store
    /// additional `data.len()` bytes.
    pub(super) unsafe fn extend_from_slice_unchecked(&mut self, data: impl AsRef<[u8]>) -> usize {
        unsafe {
            // SAFETY: This won't cause buffer overflow if safety contract is upheld.
            let write = self.data.as_mut_ptr().add(self.len);
            for (offset, &byte) in data.as_ref().iter().enumerate() {
                write.add(offset).write(byte);
            }
        }
        self.len += data.as_ref().len();
        self.capacity() - self.len()
    }
}

impl<'i> AsRef<[u8]> for OpenBucket<'i> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &Pin::get_ref(Pin::as_ref(&self.data))[..self.len]
    }
}

impl<'i> From<OpenBucket<'i>> for ClosedBucket<'i> {
    /// Turns `OpenBucket` into a `ClosedBucket` without copying the data.
    fn from(mut value: OpenBucket<'i>) -> Self {
        // Take pinned data and replace it with an empty slice
        let data = {
            let mut data: Pin<&mut [u8]> = Pin::new(&mut []);
            std::mem::swap(&mut data, &mut value.data);
            Pin::get_mut(data)
        };

        // Free any unused parts of taken data
        let (data, unused) = data.split_at_mut(value.len);
        if !unused.is_empty() {
            let layout = unsafe {
                // SAFETY: size is not 0; alignment of 1 is valid for `u8` array, and size
                //         is always multiple of 1.
                core::alloc::Layout::from_size_align_unchecked(unused.len(), 1)
            };
            unsafe {
                // SAFETY: unused is uniquely owned and layout was made using `unused.len()` size
                //         and correct alignment for `u8`.
                alloc::alloc::dealloc(unused as *mut [u8] as *mut u8, layout);
            }
        }

        // Move ownership to ClosedBucket
        ClosedBucket {
            data: Pin::new(data),
            _pinned: value._pinned,
        }
    }
}

impl<'i> Drop for OpenBucket<'i> {
    fn drop(&mut self) {
        if self.data.is_empty() {
            // Already moved or 0-allocated
            return;
        }
        
        let data = Pin::get_mut(Pin::as_mut(&mut self.data));
        let layout = unsafe {
            // SAFETY: size is not 0; alignment of 1 is valid for `u8` array, and size
            //         is always multiple of 1.
            core::alloc::Layout::from_size_align_unchecked(data.len(), 1)
        };
        unsafe {
            // SAFETY: data is uniquely owned and layout was made using `unused.len()`
            //         size and correct alignment for `u8`.
            alloc::alloc::dealloc(data as *mut [u8] as *mut u8, layout);
        }
    }
}

/// A closed bucket is an immutable sequence of bytes.
///
/// It makes same guarantees as [`OpenBucket`], except it's also immutable and can be
/// treated as a valid sequence of correctly encoded characters.
///
/// Refer to [`OpenBucket`] for more information.
///
/// # Notes
///
/// By design, a closed bucket can only be accessed or dropped (deallocating the data). It
/// can't be turned into `OpenBucket` without copying its contents.
///
/// UTF-8 encoding isn't inherent characteristic of a `ClosedBucket`, but it arises from
/// the fact that [`OpenBucket::push_str`] only accepts `str` arguments.
#[repr(transparent)]
pub struct ClosedBucket<'i> {
    // intentionally not `&'i mut str` to allow other encodings in the future
    data: Pin<&'i mut [u8]>,
    _pinned: PhantomPinned,
}

impl<'i> ClosedBucket<'i> {
    /// Returns a pointer range of bucket data.
    ///
    /// This range can be assumed to contain only UTF-8 characters.
    #[inline]
    pub fn as_ptr_range(&self) -> core::ops::Range<*const u8> {
        unsafe { self.as_ptr()..(self.as_ptr().add(self.len())) }
    }

    /// Returns a pinned `&'i str` reference to owned data.
    #[inline]
    pub fn as_str(&self) -> Pin<&'i str> {
        unsafe {
            let mapped = Pin::map_unchecked(self.data.as_ref(), |data: &[u8]| {
                // SAFETY: must be UTF-8 because data was copied from `str`
                std::str::from_utf8_unchecked(data)
            });
            // SAFETY: it's valid to extend the lifetime of this borrow because the bucket
            //         will keep the data allocated for 'i duration
            core::mem::transmute(mapped)
        }
    }
}

impl<'i> AsRef<[u8]> for ClosedBucket<'i> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        Pin::get_ref(Pin::as_ref(&self.data))
    }
}

/// All `str` methods are also valid for ClosedBucket because it is effectively a
/// borrowed string.
impl<'i> core::ops::Deref for ClosedBucket<'i> {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        Pin::get_ref(self.as_str())
    }
}

impl<'i> Drop for ClosedBucket<'i> {
    fn drop(&mut self) {
        let data = unsafe {
            // SAFETY: Referenced data is completely owned by the current function, and
            //         &mut self is consumed right after it's been turned into a &mut [u8].
            let slice = std::slice::from_raw_parts_mut(self.as_ptr() as *mut u8, self.len());
            let _ = self; // consume self; unique ownership
            slice
        };
        let layout = unsafe {
            // SAFETY: size constraints checked in constructor; alignment of 1 is valid
            //         for `u8`, and size is multiple of 1.
            core::alloc::Layout::from_size_align_unchecked(data.len(), 1)
        };
        unsafe {
            // SAFETY: data is uniquely owned and layout was made using `data.len()` size
            //         and correct alignment for `u8`.
            alloc::alloc::dealloc(data as *mut [u8] as *mut u8, layout);
        }
    }
}

impl<'i> Debug for ClosedBucket<'i> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ClosedBucket")
            .field("data", &&self.data[..])
            .finish()
    }
}

/// Error returned by [`OpenBucket::push_str`] when there's not enough space to push a string.
#[derive(Debug)]
pub struct ExceedsCapacityError {
    requested: usize,
    remaining: usize,
}
impl core::fmt::Display for ExceedsCapacityError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "pushing {} bytes would exceed bucket capacity; remaining space: {}",
            self.requested, self.remaining
        )
    }
}
impl core::error::Error for ExceedsCapacityError {}
