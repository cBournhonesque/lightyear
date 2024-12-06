//! Vendored version of crate `object-pool` with updated parking_lot dependency for wasm support

//! A thread-safe object pool with automatic return and attach/detach semantics
//!
//! The goal of an object pool is to reuse expensive to allocate objects or frequently allocated objects
//!
//! # Examples
//!
//! ## Creating a Pool
//!
//! The general pool creation looks like this
//! ```ignore
//! # use object_pool::Pool;
//! # type T = Vec<u32>;
//! # const capacity: usize = 5;
//!  let pool: Pool<T> = Pool::new(capacity, || T::new());
//! ```
//! Example pool with 32 `Vec<u8>` with capacity of 4096
//! ```ignore
//! # use object_pool::Pool;
//!  let pool: Pool<Vec<u8>> = Pool::new(32, || Vec::with_capacity(4096));
//! ```
//!
//! ## Using a Pool
//!
//! Basic usage for pulling from the pool
//! ```ignore
//! # use object_pool::Pool;
//! # use std::io::Read;
//! # let mut some_file = std::fs::File::open("/dev/null").unwrap();
//! let pool: Pool<Vec<u8>> = Pool::new(32, || Vec::with_capacity(4096));
//! let mut reusable_buff = pool.try_pull().unwrap(); // returns None when the pool is saturated
//! reusable_buff.clear(); // clear the buff before using
//! some_file.read_to_end(&mut reusable_buff).ok();
//! // reusable_buff is automatically returned to the pool when it goes out of scope
//! ```
//! Pull from pool and `detach()`
//! ```ignore
//! # use object_pool::Pool;
//! let pool: Pool<Vec<u8>> = Pool::new(32, || Vec::with_capacity(4096));
//! let mut reusable_buff = pool.try_pull().unwrap(); // returns None when the pool is saturated
//! reusable_buff.clear(); // clear the buff before using
//! let (pool, reusable_buff) = reusable_buff.detach();
//! let mut s = String::from_utf8(reusable_buff).unwrap();
//! s.push_str("hello, world!");
//! pool.attach(s.into_bytes()); // reattach the buffer before reusable goes out of scope
//! // reusable_buff is automatically returned to the pool when it goes out of scope
//! ```
//!
//! ## Using Across Threads
//!
//! You simply wrap the pool in a [`std::sync::Arc`]
//! ```ignore
//! # use std::sync::Arc;
//! # use object_pool::Pool;
//! # type T = Vec<u32>;
//! # const cap: usize = 5;
//! let pool: Arc<Pool<T>> = Arc::new(Pool::new(cap, || T::new()));
//! ```
//!
//! # Warning
//!
//! Objects in the pool are not automatically reset, they are returned but NOT reset
//! You may want to call `object.reset()` or  `object.clear()`
//! or any other equivalent for the object that you are using, after pulling from the pool
//!
//! [`std::sync::Arc`]: https://doc.rust-lang.org/stable/std/sync/struct.Arc.html

use std::iter::FromIterator;
use std::mem::{forget, ManuallyDrop};
use std::ops::{Deref, DerefMut};

use parking_lot::Mutex;

pub type Stack<T> = Vec<T>;

pub struct Pool<T> {
    objects: Mutex<Stack<T>>,
}

impl<T> Pool<T> {
    #[inline]
    pub fn new<F>(cap: usize, init: F) -> Pool<T>
    where
        F: Fn() -> T,
    {
        Pool {
            objects: Mutex::new((0..cap).map(|_| init()).collect()),
        }
    }

    #[inline]
    pub fn from_vec(v: Vec<T>) -> Pool<T> {
        Pool {
            objects: Mutex::new(v),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.objects.lock().len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.objects.lock().is_empty()
    }

    #[inline]
    pub fn try_pull(&self) -> Option<Reusable<T>> {
        self.objects
            .lock()
            .pop()
            .map(|data| Reusable::new(self, data))
    }

    #[inline]
    pub fn pull<F: Fn() -> T>(&self, fallback: F) -> Reusable<T> {
        self.try_pull()
            .unwrap_or_else(|| Reusable::new(self, fallback()))
    }

    #[inline]
    pub fn attach(&self, t: T) {
        self.objects.lock().push(t)
    }
}

impl<T> FromIterator<T> for Pool<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self {
            objects: Mutex::new(iter.into_iter().collect()),
        }
    }
}

pub struct Reusable<'a, T> {
    pool: &'a Pool<T>,
    data: ManuallyDrop<T>,
}

impl<'a, T> Reusable<'a, T> {
    #[inline]
    pub fn new(pool: &'a Pool<T>, t: T) -> Self {
        Self {
            pool,
            data: ManuallyDrop::new(t),
        }
    }

    #[inline]
    pub fn detach(mut self) -> (&'a Pool<T>, T) {
        let ret = unsafe { (self.pool, self.take()) };
        forget(self);
        ret
    }

    unsafe fn take(&mut self) -> T {
        ManuallyDrop::take(&mut self.data)
    }
}

impl<T> Deref for Reusable<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> DerefMut for Reusable<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<T> Drop for Reusable<'_, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe { self.pool.attach(self.take()) }
    }
}

#[cfg(test)]
mod tests {
    use std::mem::drop;

    use super::{Pool, Reusable};

    #[test]
    fn detach() {
        let pool = Pool::new(1, Vec::new);
        let (pool, mut object) = pool.try_pull().unwrap().detach();
        object.push(1);
        Reusable::new(pool, object);
        assert_eq!(pool.try_pull().unwrap()[0], 1);
    }

    #[test]
    fn detach_then_attach() {
        let pool = Pool::new(1, Vec::new);
        let (pool, mut object) = pool.try_pull().unwrap().detach();
        object.push(1);
        pool.attach(object);
        assert_eq!(pool.try_pull().unwrap()[0], 1);
    }

    #[test]
    fn pull() {
        let pool = Pool::<Vec<u8>>::new(1, Vec::new);

        let object1 = pool.try_pull();
        let object2 = pool.try_pull();
        let object3 = pool.pull(Vec::new);

        assert!(object1.is_some());
        assert!(object2.is_none());
        drop(object1);
        drop(object2);
        drop(object3);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn e2e() {
        let pool = Pool::new(10, Vec::new);
        let mut objects = Vec::new();

        for i in 0..10 {
            let mut object = pool.try_pull().unwrap();
            object.push(i);
            objects.push(object);
        }

        assert!(pool.try_pull().is_none());
        drop(objects);
        assert!(pool.try_pull().is_some());

        for i in (0..10).rev() {
            let mut object = pool.objects.lock().pop().unwrap();
            assert_eq!(object.pop(), Some(i));
        }
    }
}
