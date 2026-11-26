use std::fmt;
use std::ops::{Add, Sub};

/// A node index in [`super::Document::nodes`].
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct NodeId(u32);

/// A tag-token index in [`super::tag::TagTable`].
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TagId(u32);

macro_rules! id_type {
    ($name:ident) => {
        impl $name {
            #[inline(always)]
            pub const fn new(index: u32) -> Self {
                Self(index)
            }

            #[inline(always)]
            pub const fn get(self) -> u32 {
                self.0
            }

            #[inline(always)]
            pub const fn index(self) -> usize {
                self.0 as usize
            }

            #[inline(always)]
            pub const fn is_none(self) -> bool {
                self.0 == u32::MAX
            }
        }

        impl From<u32> for $name {
            #[inline(always)]
            fn from(index: u32) -> Self {
                Self(index)
            }
        }

        impl From<$name> for u32 {
            #[inline(always)]
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl From<$name> for usize {
            #[inline(always)]
            fn from(id: $name) -> Self {
                id.index()
            }
        }

        impl From<usize> for $name {
            #[inline(always)]
            fn from(index: usize) -> Self {
                Self(index as u32)
            }
        }

        impl Add<u32> for $name {
            type Output = Self;

            #[inline(always)]
            fn add(self, rhs: u32) -> Self {
                Self(self.0 + rhs)
            }
        }

        impl Sub<u32> for $name {
            type Output = Self;

            #[inline(always)]
            fn sub(self, rhs: u32) -> Self {
                Self(self.0 - rhs)
            }
        }
    };
}

id_type!(NodeId);
id_type!(TagId);

pub const NO_NODE: NodeId = NodeId::new(u32::MAX);
pub const NO_TAG: TagId = TagId::new(u32::MAX);
pub const NO_ATTR: u32 = u32::MAX;

/// The document node: always node `0`, and the only node without a parent.
pub const DOC_ROOT: NodeId = NodeId::new(0);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for TagId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
