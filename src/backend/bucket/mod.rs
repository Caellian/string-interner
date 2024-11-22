#![cfg(feature = "backends")]

// "backend::bucket" uses "bucket"s
#[allow(clippy::module_inception)]
mod bucket;
mod interned_str;

use self::{bucket::OpenBucket, interned_str::InternedStr};
use super::{Backend, PhantomBackend};
use crate::{
    symbol::expect_valid_symbol,
    DefaultSymbol, Symbol,
};
use alloc::vec::Vec;
use bucket::ClosedBucket;
use core::ops::Add;

/// Average length of 1 English word (5ch), rounded up to 2 multiple.
///
/// This is used as expected average span length because interned strings in many usecases
/// are keywords/class names/similar, which often consist of one or two words.
const AVG_WORD_LENGTH: usize = 8;

/// An interner backend that reduces memory allocations by using buckets.
///
/// # Overview
/// This interner uses fixed-size buckets to store interned strings. Each bucket is
/// allocated once and holds a set number of strings. When a bucket becomes full, a new
/// bucket is allocated to hold more strings. Buckets are never deallocated, which reduces
/// the overhead of frequent memory allocations and copying.
///
/// ## Trade-offs
/// - **Advantages:**
///   - Strings in already used buckets remain valid and accessible even as new strings
///     are added.
/// - **Disadvantages:**
///   - Slightly slower access times due to double indirection (looking up the string
///     involves an extra level of lookup through the bucket).
///   - Memory may be used inefficiently if many buckets are allocated but only partially
///     filled because of large strings.
///
/// ## Use Cases
/// This backend is ideal when interned strings must remain valid even after new ones are
/// added.general use
///
/// Refer to the [comparison table][crate::_docs::comparison_table] for comparison with
/// other backends.
///
/// [matklad's blog post]:
///     https://matklad.github.io/2020/03/22/fast-simple-rust-interner.html
#[derive(Debug)]
pub struct BucketBackend<'i, S: Symbol = DefaultSymbol> {
    spans: Vec<InternedStr<'i>>,
    head: Option<OpenBucket<'i>>,
    full: Vec<ClosedBucket<'i>>,
    marker: PhantomBackend<'i, Self>,
}

/// # Safety
///
/// The bucket backend requires a manual [`Send`] impl because it is self
/// referential. When cloning a bucket backend a deep clone is performed and
/// all references to itself are updated for the clone.
unsafe impl<'i, S> Send for BucketBackend<'i, S> where S: Symbol {}

/// # Safety
///
/// The bucket backend requires a manual [`Send`] impl because it is self
/// referential. Those references won't escape its own scope and also
/// the bucket backend has no interior mutability.
unsafe impl<'i, S> Sync for BucketBackend<'i, S> where S: Symbol {}

impl<'i, S: Symbol> Default for BucketBackend<'i, S> {
    #[cfg_attr(feature = "inline-more", inline)]
    fn default() -> Self {
        // Using some ~sensible defaults to reduce reallocations.
        Self {
            spans: Vec::with_capacity(32), // 0.5 KiB
            head: None,
            full: Vec::with_capacity(8), // 128 B
            marker: Default::default(),
        }
    }
}

impl<'i, S> Backend<'i> for BucketBackend<'i, S>
where
    S: Symbol,
{
    type Access<'local> = &'i str
    where
        Self: 'local,
        'i: 'local;
    type Symbol = S;
    type Iter<'l>
        = Iter<'i, 'l, S>
    where
        Self: 'l;

    #[cfg_attr(feature = "inline-more", inline)]
    fn with_capacity(capacity: usize) -> Self {
        Self {
            spans: Vec::with_capacity((capacity / AVG_WORD_LENGTH).next_power_of_two()),
            head: Some(OpenBucket::with_capacity(capacity)),
            full: Vec::with_capacity(8),
            marker: Default::default(),
        }
    }

    #[inline]
    fn intern(&mut self, string: &str) -> Self::Symbol {
        let interned = self.alloc(string);
        self.push_span(interned)
    }

    #[inline]
    fn intern_static(&mut self, string: &'static str) -> Self::Symbol {
        let interned = InternedStr::new_static(string);
        self.push_span(interned)
    }

    fn shrink_to_fit(&mut self) {
        self.spans.shrink_to_fit();
        self.full.shrink_to_fit();
    }

    #[inline]
    fn resolve(&self, symbol: Self::Symbol) -> Option<&'i str> {
        self.spans.get(symbol.to_usize()).map(InternedStr::as_str)
    }

    #[inline]
    unsafe fn resolve_unchecked(&self, symbol: Self::Symbol) -> &'i str {
        // SAFETY: The function is marked unsafe so that the caller guarantees
        //         that required invariants are checked.
        unsafe { self.spans.get_unchecked(symbol.to_usize()).as_str() }
    }

    #[inline]
    fn iter(&self) -> Self::Iter<'_> {
        Iter::new(self)
    }
}

impl<'i, S> BucketBackend<'i, S>
where
    S: Symbol,
{
    /// Creates a new bucket backend.
    pub fn new(span_capacity: usize, bucket_capacity: usize, expect_bucket_count: usize) -> Self {
        Self {
            spans: Vec::with_capacity(span_capacity),
            head: Some(OpenBucket::with_capacity(bucket_capacity)),
            full: Vec::with_capacity(expect_bucket_count),
            marker: Default::default(),
        }
    }

    /// Returns the next available symbol.
    fn next_symbol(&self) -> S {
        expect_valid_symbol(self.spans.len())
    }

    /// Pushes the given interned string into the spans and returns its symbol.
    fn push_span(&mut self, interned: InternedStr<'i>) -> S {
        let symbol = self.next_symbol();
        self.spans.push(interned);
        symbol
    }

    fn next_head_capacity(&self, at_least: usize) -> usize {
        self.head
            .as_ref()
            .map(OpenBucket::capacity)
            .unwrap_or_default()
            .max(at_least)
            .add(1)
            .next_power_of_two()
    }

    /// Creates a new head with specified capacity, and finalizes the previous one.
    fn new_head(&mut self, capacity: usize) -> &mut OpenBucket<'i> {
        let created = OpenBucket::with_capacity(capacity);
        if let Some(head) = &mut self.head {
            let previous = core::mem::replace(head, created);
            self.full.push(previous.into());
            return unsafe {
                // SAFETY: A borrow of bucket is not related to interner duration 'i
                std::mem::transmute::<&mut OpenBucket<'_>, &mut OpenBucket<'i>>(head)
            };
        }
        self.head = Some(created);
        unsafe {
            // SAFETY: Head was just created.
            self.head.as_mut().unwrap_unchecked()
        }
    }

    /// Interns a new string into the backend and returns a reference to it.
    fn alloc(&mut self, string: &str) -> InternedStr<'i> {
        let head = match &mut self.head {
            Some(it) if it.can_store(string.len()) => it,
            _ => self.new_head(self.next_head_capacity(string.len())),
        };
        head.push_str(string).unwrap()
    }
}

impl<'i, S: Symbol> Clone for BucketBackend<'i, S> {
    fn clone(&self) -> Self {
        // If head is None, there's no buckets allocated and BucketBackend::new should work.
        // This assumption has been ignored though to allow weird cases in future.

        // New head size will be equal to current one to avoid overallocation.
        let head = self
            .head
            .as_ref()
            .map(|it| OpenBucket::with_capacity(it.capacity()));

        // Collect a list of section memory ranges
        let sections = {
            let mut sections: Vec<_> = self.full.iter().map(ClosedBucket::as_ptr_range).collect();
            if let Some(head) = &self.head {
                sections.push(head.as_ptr_range());
            }
            sections
        };

        // Collect global offests of all sections if they were put one after another
        let (preceeding_jumps, total_size): (Vec<usize>, usize) = {
            let (mut ends, mut total) = self.full.iter().map(|it| it.len()).fold(
                (Vec::with_capacity(sections.len()), 0),
                |(mut acc, total), it| {
                    acc.push(acc.iter().cloned().sum::<usize>() + it);
                    (acc, total + it)
                },
            );
            match &self.head {
                Some(head) => {
                    // include head size in total
                    total += head.len();
                }
                None => {
                    // last end is unused if there's no head
                    ends.pop();
                }
            }
            // excludes first jump (=0) to avoid moving all vec values
            (ends, total)
        };

        let span_offsets: Vec<_> = self
            .spans
            .iter()
            .map(|span| {
                let pos = span.as_ptr();
                match sections
                    .iter()
                    .enumerate()
                    .find(|(_, section)| section.contains(&pos))
                {
                    Some((i, owned)) => {
                        let global_offset = if i == 0 {
                            // first jump is excluded
                            0
                        } else {
                            unsafe {
                                // SAFETY: iterator produced from self.full must contain
                                //         same number of elements as the other (excluding
                                //         the missing i==0 one, which is checked)
                                *preceeding_jumps.get_unchecked(
                                    // SAFETY: checked i != 0
                                    i.unchecked_sub(1),
                                )
                            }
                        };
                        let local_offset = pos as usize - owned.start as usize;
                        (Ok(global_offset + local_offset), span.len())
                    }
                    None => {
                        // a 'static span
                        (Err(span.as_ptr()), span.len())
                    }
                }
            })
            .collect();

        let full: ClosedBucket = unsafe {
            // SAFETY: unchecked extend is safe because total_size includes sizes of all
            //         full buckets and head (if present)

            let mut full = OpenBucket::with_capacity(total_size);
            for bucket in &self.full {
                full.extend_from_slice_unchecked(bucket);
            }
            if let Some(head) = &self.head {
                full.extend_from_slice_unchecked(head);
            }
            full.into()
        };

        let spans: Vec<_> = span_offsets
            .into_iter()
            .map(|(offset, length)| {
                let position = match offset {
                    Ok(offset) => unsafe { full.as_ptr().add(offset) },
                    Err(static_offset) => static_offset,
                };
                unsafe {
                    // SAFETY:
                    // - `position` points to newly created `full` bucket, so it's uniquely owned
                    // - `position` points to a valid UTF-8 string
                    // - pointed-to string is of provided `length`
                    InternedStr::from_raw_parts(position, length)
                }
            })
            .collect();

        Self {
            spans,
            head,
            full: vec![full],
            marker: Default::default(),
        }
    }
}

impl<'i, S> Eq for BucketBackend<'i, S> where S: Symbol {}

impl<'i, S> PartialEq for BucketBackend<'i, S>
where
    S: Symbol,
{
    #[cfg_attr(feature = "inline-more", inline)]
    fn eq(&self, other: &Self) -> bool {
        // FIXME: Incorrect and expensive
        self.spans == other.spans
    }
}

impl<'i, 'l, S> IntoIterator for &'l BucketBackend<'i, S>
where
    S: Symbol,
{
    type Item = (S, &'i str);
    type IntoIter = Iter<'i, 'l, S>;

    #[cfg_attr(feature = "inline-more", inline)]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct Iter<'i, 'l, S: Symbol> {
    backend: &'l BucketBackend<'i, S>,
    /// Span to be produced next.
    current_span: usize,
    /// Available spans at the time of iterator creation.
    spans: std::ops::Range<usize>,
}

impl<'i, 'l, S: Symbol> Iter<'i, 'l, S>
where
    'i: 'l,
{
    #[cfg_attr(feature = "inline-more", inline)]
    pub fn new(backend: &'l BucketBackend<'i, S>) -> Self {
        Self {
            backend,
            current_span: 0,
            spans: 0..backend.spans.len(),
        }
    }
}

impl<'i, 'l, S> Iterator for Iter<'i, 'l, S>
where
    S: Symbol,
{
    type Item = (S, &'i str);

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.spans.size_hint()
    }

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if !self.spans.contains(&self.current_span) {
            return None;
        }
        let span = self.current_span;
        let symbol = expect_valid_symbol(span);
        let span = unsafe {
            // SAFETY: Only new items can be added to spans, so any previously valid index
            //         will always be valid.
            self.backend.spans.get_unchecked(span)
        };
        self.current_span += 1;

        Some((symbol, span.as_str()))
    }
}
