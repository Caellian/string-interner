use core::pin::Pin;

/// Reference to an interned string.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct InternedStr<'i>(Pin<&'i str>);

impl<'i> InternedStr<'i> {
    /// Creates a new interned string from provided string [`Pin`].
    #[inline]
    pub fn new(value: Pin<&'i str>) -> Self {
        InternedStr(value)
    }

    /// Creates a new interned string from a static string.
    #[inline]
    pub fn new_static(value: &'static str) -> Self {
        InternedStr(Pin::new(value))
    }

    /// Creates a new interned string from string pointer and length.
    /// 
    /// # Safety
    /// 
    /// This function is safe to call under following conditions:
    /// - `position` is not NULL,
    /// - `position` must point to a valid UTF-8 sequence of bytes with provided `length`,
    /// - pointed-to `str` must exist for 'i duration (or longer)
    ///   - that is, it must be owned by bucket interner (unless it's static).
    pub(in super) unsafe fn from_raw_parts(position: *const u8, length: usize) -> Self {
        let string = unsafe {
            // SAFETY: `position` points to non-null address of provided `length` by contract.
            std::slice::from_raw_parts(position, length)
        };
        let string = unsafe {
            // SAFETY: `string` slice is a valid UTF-8 string by contract.
            std::str::from_utf8_unchecked(string)
        };
        Self::new(Pin::new(string))
    }
    
    /// Returns a reference to interned string.
    pub fn as_str(&self) -> &'i str {
        unsafe {
            // SAFETY: It's safe to extend lifetime of borrow because interned string will
            //         be valid for 'i, regardless of what happens to this wrapper.
            std::mem::transmute::<&str, &'i str>(&self.0)
        }
    }
}

impl<'i> PartialEq for InternedStr<'i> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_ptr() == other.as_ptr()
    }
}
impl<'i> Eq for InternedStr<'i> {}

impl<'i> PartialEq<str> for InternedStr<'i> {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_ref() == other
    }
}
impl<'i> PartialEq<InternedStr<'i>> for str {
    #[inline]
    fn eq(&self, other: &InternedStr<'i>) -> bool {
        self == other.as_ref()
    }
}

impl<'i> AsRef<str> for InternedStr<'i> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<'i> core::ops::Deref for InternedStr<'i> {
    type Target = Pin<&'i str>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
