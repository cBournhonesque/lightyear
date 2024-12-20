#[derive(Debug, Clone, Copy)]
pub struct FreeList<T: Sized, const N: usize> {
    len: usize,
    inner: [Option<T>; N],
}

pub struct FreeListIter<'a, T: Sized + Copy, const N: usize> {
    pub free_list: &'a FreeList<T, N>,
    pub index: usize,
}

impl<T: Sized + Copy, const N: usize> FreeList<T, N> {
    pub fn new() -> Self {
        Self {
            len: 0,
            inner: [None; N],
        }
    }
}

impl<T: Sized, const N: usize> FreeList<T, N> {
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn insert(&mut self, value: T) -> usize {
        if self.len >= N {
            panic!("free list is full");
        }
        // Find the first empty slot, insert the value there, and return the index
        let index = self
            .inner
            .iter()
            .position(|x| x.is_none())
            .expect("len should be less than N");
        self.len += 1;
        self.inner[index] = Some(value);
        index
    }

    pub fn remove(&mut self, index: usize) {
        if self.inner[index].is_some() {
            self.len -= 1;
            self.inner[index] = None;
        }
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.inner.get(index).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.inner.get_mut(index).and_then(Option::as_mut)
    }

    pub fn iter(&self) -> FreeListIter<'_, T, N>
    where
        T: Copy,
    {
        FreeListIter {
            free_list: self,
            index: 0,
        }
    }
}

impl<T: Sized, const N: usize> std::ops::Index<usize> for FreeList<T, N> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("index out of bounds")
    }
}

impl<T: Sized, const N: usize> std::ops::IndexMut<usize> for FreeList<T, N> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.get_mut(index).expect("index out of bounds")
    }
}

impl<T: Sized + Copy, const N: usize> Iterator for FreeListIter<'_, T, N> {
    type Item = (usize, T);

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < N {
            let index = self.index;
            self.index += 1;
            if let Some(value) = self.free_list.get(index) {
                return Some((index, *value));
            }
        }
        None
    }
}
