//! Interfaces and types to be used as symbols for the
//! [`StringInterner`](`crate::StringInterner`).
//!
//! The [`StringInterner::get_or_intern`](`crate::StringInterner::get_or_intern`)
//! method returns `Symbol` types that allow to look-up the original string
//! using [`StringInterner::resolve`](`crate::StringInterner::resolve`).

/// Types implementing this trait can be used as symbols for string interners.
///
/// The [`StringInterner::get_or_intern`](`crate::StringInterner::get_or_intern`)
/// method returns `Symbol` types that allow to look-up the original string
/// using [`StringInterner::resolve`](`crate::StringInterner::resolve`).
///
/// # Note
///
/// Optimal symbols allow for efficient comparisons and have a small memory footprint.
pub trait Symbol: Copy + Eq + TryFrom<usize> + Into<usize>
{
    /// Produces a symbol from
    /// 
    /// # Safety
    ///
    /// Caller must ensure `index` doesn't excede numeric limitations of this type.
    /// 
    /// # Implementation
    /// 
    /// Default implementation simply unwraps the result of [`TryFrom`]. Implementors are
    /// encouraged to add conversion logic to this method and call it from `TryFrom`
    /// instead of other way around (i.e. the default), so that backends which know
    /// certain indices are valid can avoid overhead of checking and unwrapping `Result`.
    #[inline]
    unsafe fn from_usize_unchecked(index: usize) -> Self {
        unsafe {
            Self::try_from(index).unwrap_unchecked()
        }
    }
}

/// Creates the symbol `S` from the given `usize`.
///
/// # Panics
///
/// Panics if the conversion is invalid.
#[cfg(feature = "backends")]
#[inline]
pub(crate) fn expect_valid_symbol<S>(index: usize) -> S
where
    S: Symbol,
{
    match S::try_from(index) {
        Ok(it) => it,
        Err(_) => panic!("{index} not a valid symbol")
    }
}

/// Creates the symbol `S` from the given `usize` without checking whether it's valid.
///
/// This is useful for cases where index is known to be valid (such as from iterators).
///
/// # Safety
///
/// Provided `index` must be convertible to `S`.
#[cfg(feature = "backends")]
#[inline]
pub(crate) unsafe fn assume_valid_symbol<S>(index: usize) -> S
where
    S: Symbol,
{
    unsafe { S::from_usize_unchecked(index) }
}

/// The symbol type that is used by default.
pub type DefaultSymbol = SymbolU32;

impl Symbol for usize {}

macro_rules! gen_symbol_for {
    (
        $( #[$doc:meta] )*
        struct $name:ident($base_ty: ty);
    ) => {
        $( #[$doc] )*
        #[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name {
            value: core::num::NonZero<$base_ty>,
        }

        impl Symbol for $name {
            #[inline]
            unsafe fn from_usize_unchecked(index: usize) -> Self {
                Self {
                    value: unsafe {
                        // SAFETY: NonZero construction can never fail because index is
                        //         unsigned and incremented by one.
                        <core::num::NonZero<$base_ty>>::new_unchecked((index as $base_ty).wrapping_add(1))
                    }
                }
            }
        }

        impl TryFrom<usize> for $name {
            type Error = OutOfBoundsError;

            fn try_from(value: usize) -> Result<Self, Self::Error> {
                if value >= <$base_ty>::MAX as usize {
                    return Err(OutOfBoundsError {
                        got: value,
                        max: <$base_ty>::MAX as usize
                    });
                }
                Ok(unsafe {
                    // SAFETY: Value has been checked.
                    Self::from_usize_unchecked(value)
                })
            }
        }

        impl From<$name> for usize {
            #[inline]
            fn from(value: $name) -> usize {
                value.value.get() as usize - 1
            }
        }
    };
}
gen_symbol_for!(
    /// Symbol that is 16-bit in size.
    ///
    /// Is space-optimized for used in `Option`.
    struct SymbolU16(u16);
);
gen_symbol_for!(
    /// Symbol that is 32-bit in size.
    ///
    /// Is space-optimized for used in `Option`.
    struct SymbolU32(u32);
);
gen_symbol_for!(
    /// Symbol that is the same size as a pointer (`usize`).
    ///
    /// Is space-optimized for used in `Option`.
    struct SymbolUsize(usize);
);

/// Error returned when a Symbol value is out of bounds.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct OutOfBoundsError {
    got: usize,
    max: usize,
}
impl core::fmt::Display for OutOfBoundsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "symbol value ({}) out of bounds; max allowed value is: {}",
            self.got, self.max
        )
    }
}
impl core::error::Error for OutOfBoundsError {}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;
    use core::num::{NonZeroU16, NonZeroU32, NonZeroUsize};

    #[test]
    fn same_size_as_u32() {
        assert_eq!(size_of::<DefaultSymbol>(), size_of::<u32>());
    }

    #[test]
    fn same_size_as_optional() {
        assert_eq!(
            size_of::<DefaultSymbol>(),
            size_of::<Option<DefaultSymbol>>()
        );
    }

    #[test]
    fn try_from_usize_works() {
        assert_eq!(
            SymbolU16::try_from(0),
            Ok(SymbolU16 {
                value: NonZeroU16::new(1).unwrap()
            })
        );
        assert_eq!(
            SymbolU16::try_from(u16::MAX as usize - 1),
            Ok(SymbolU16 {
                value: NonZeroU16::new(u16::MAX).unwrap()
            })
        );
        assert!(SymbolU16::try_from(u16::MAX as usize).is_err());
        assert!(SymbolU16::try_from(usize::MAX).is_err());
    }

    macro_rules! gen_test_for {
        ( $test_name:ident: struct $name:ident($non_zero:ty; $base_ty:ty); ) => {
            #[test]
            fn $test_name() {
                for val in 0..10 {
                    assert_eq!(
                        <$name>::try_from(val),
                        Ok($name {
                            value: <$non_zero>::new(val as $base_ty + 1).unwrap()
                        })
                    );
                }
                assert_eq!(
                    <$name>::try_from(<$base_ty>::MAX as usize - 1),
                    Ok($name {
                        value: <$non_zero>::new(<$base_ty>::MAX).unwrap()
                    })
                );
                assert!(<$name>::try_from(<$base_ty>::MAX as usize).is_err());
                assert!(<$name>::try_from(<usize>::MAX).is_err());
            }
        };
    }
    gen_test_for!(
        try_from_usize_works_for_u16:
        struct SymbolU16(NonZeroU16; u16);
    );
    gen_test_for!(
        try_from_usize_works_for_u32:
        struct SymbolU32(NonZeroU32; u32);
    );
    gen_test_for!(
        try_from_usize_works_for_usize:
        struct SymbolUsize(NonZeroUsize; usize);
    );
}
