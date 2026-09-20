#![allow(dead_code)]

use num_traits::PrimInt;

#[derive(Debug, Default, Clone, Copy)]
pub struct Iota<T: PrimInt> {
    start: T,
    count: usize,
}

impl<T: PrimInt> Iota<T> {
    pub fn new(start: T, count: usize) -> Self {
        Self { start, count }
    }

    pub fn start(&self) -> T {
        self.start
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn with_count(self, count: usize) -> Self {
        Self {
            start: self.start,
            count,
        }
    }

    pub fn as_range(&self) -> std::ops::Range<usize>
    where
        T: Into<usize> + Copy,
    {
        let start: usize = self.start.into();
        start..start + self.count
    }
}

impl<T: PrimInt> IntoIterator for Iota<T> {
    type Item = T;
    type IntoIter = IotaIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IotaIter {
            current: self.start,
            remaining: self.count,
        }
    }
}

pub struct IotaIter<T: PrimInt> {
    current: T,
    remaining: usize,
}

impl<T: PrimInt> Iterator for IotaIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            None
        } else {
            let val = self.current;
            self.current = self.current + T::one();
            self.remaining -= 1;
            Some(val)
        }
    }
}
