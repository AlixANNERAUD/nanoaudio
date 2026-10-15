#![no_std]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

use core::{
    cell::UnsafeCell,
    slice,
    sync::atomic::{AtomicUsize, Ordering},
};

/// A lock-free ring buffer of `N` bytes, shared by one [`Producer`] and one [`Consumer`].
///
/// [`split`](Self::split) hands out the two halves, which can live in different tasks or interrupts. Both work on contiguous slices: a grant or a readable slice stops at the end of the storage, and the rest comes with the next call. `N` must be between 1 and `usize::MAX / 2`, which is checked at compile time.
pub struct RingBuffer<const N: usize> {
    storage: UnsafeCell<[u8; N]>,
    read: AtomicUsize,
    write: AtomicUsize,
}

unsafe impl<const N: usize> Sync for RingBuffer<N> {}

impl<const N: usize> RingBuffer<N> {
    const CAPACITY_IS_VALID: () = assert!(N > 0 && N <= usize::MAX / 2);

    /// Creates an empty buffer.
    pub const fn new() -> Self {
        let () = Self::CAPACITY_IS_VALID;

        Self {
            storage: UnsafeCell::new([0; N]),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
        }
    }

    /// Returns the capacity in bytes, `N`.
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Returns the number of bytes committed and not yet released.
    pub fn len(&self) -> usize {
        Self::distance(
            self.read.load(Ordering::Acquire),
            self.write.load(Ordering::Acquire),
        )
    }

    /// Returns `true` if there is nothing to read.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `true` if there is no free space.
    pub fn is_full(&self) -> bool {
        self.len() == N
    }

    /// Empties the buffer and splits it into its producer and consumer.
    ///
    /// The mutable borrow allows only one producer and one consumer at a time.
    pub fn split(&mut self) -> (Producer<'_, N>, Consumer<'_, N>) {
        *self.read.get_mut() = 0;
        *self.write.get_mut() = 0;

        (Producer { buffer: self }, Consumer { buffer: self })
    }

    const fn distance(from: usize, to: usize) -> usize {
        (to + 2 * N - from) % (2 * N)
    }

    const fn advance(index: usize, count: usize) -> usize {
        (index + count) % (2 * N)
    }

    fn pointer(&self, index: usize) -> *mut u8 {
        unsafe { self.storage.get().cast::<u8>().add(index % N) }
    }
}

impl<const N: usize> Default for RingBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// The writing half of a [`RingBuffer`].
pub struct Producer<'a, const N: usize> {
    buffer: &'a RingBuffer<N>,
}

impl<const N: usize> Producer<'_, N> {
    /// Returns the number of bytes that can be written.
    pub fn free(&self) -> usize {
        N - self.buffer.len()
    }

    /// Returns the contiguous free space at the write position.
    ///
    /// It can be shorter than [`free`](Self::free) when the free space wraps around the end of the storage. Write into it, then [`commit`](Self::commit) the bytes written.
    pub fn grant(&mut self) -> &mut [u8] {
        let read = self.buffer.read.load(Ordering::Acquire);
        let write = self.buffer.write.load(Ordering::Relaxed);
        let free = N - RingBuffer::<N>::distance(read, write);
        let length = free.min(N - write % N);

        unsafe { slice::from_raw_parts_mut(self.buffer.pointer(write), length) }
    }

    /// Makes the first `count` bytes of the grant readable, clamped to the free space.
    pub fn commit(&mut self, count: usize) {
        let count = count.min(self.free());
        let write = self.buffer.write.load(Ordering::Relaxed);

        self.buffer
            .write
            .store(RingBuffer::<N>::advance(write, count), Ordering::Release);
    }

    /// Copies as much of `data` as fits and returns the number of bytes written.
    pub fn write(&mut self, mut data: &[u8]) -> usize {
        let mut total = 0;

        while !data.is_empty() {
            let grant = self.grant();
            let count = grant.len().min(data.len());

            if count == 0 {
                break;
            }

            grant[..count].copy_from_slice(&data[..count]);
            self.commit(count);

            data = &data[count..];
            total += count;
        }

        total
    }
}

/// The reading half of a [`RingBuffer`].
pub struct Consumer<'a, const N: usize> {
    buffer: &'a RingBuffer<N>,
}

impl<const N: usize> Consumer<'_, N> {
    /// Returns the number of bytes ready to read.
    pub fn available(&self) -> usize {
        self.buffer.len()
    }

    /// Returns the contiguous readable bytes at the read position.
    ///
    /// It can be shorter than [`available`](Self::available) when the data wraps around the end of the storage. [`release`](Self::release) the bytes once they are used.
    pub fn readable(&mut self) -> &[u8] {
        let read = self.buffer.read.load(Ordering::Relaxed);
        let write = self.buffer.write.load(Ordering::Acquire);
        let available = RingBuffer::<N>::distance(read, write);
        let length = available.min(N - read % N);

        unsafe { slice::from_raw_parts(self.buffer.pointer(read), length) }
    }

    /// Frees the first `count` readable bytes, clamped to what is available.
    pub fn release(&mut self, count: usize) {
        let count = count.min(self.available());
        let read = self.buffer.read.load(Ordering::Relaxed);

        self.buffer
            .read
            .store(RingBuffer::<N>::advance(read, count), Ordering::Release);
    }

    /// Copies as many readable bytes as fit into `data` and returns the number of bytes read.
    pub fn read(&mut self, mut data: &mut [u8]) -> usize {
        let mut total = 0;

        while !data.is_empty() {
            let readable = self.readable();
            let count = readable.len().min(data.len());

            if count == 0 {
                break;
            }

            data[..count].copy_from_slice(&readable[..count]);
            self.release(count);

            data = &mut data[count..];
            total += count;
        }

        total
    }

    /// Discards everything committed so far.
    pub fn clear(&mut self) {
        let write = self.buffer.write.load(Ordering::Acquire);

        self.buffer.read.store(write, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[test]
    fn write_then_read() {
        let mut buffer = RingBuffer::<8>::new();
        let (mut producer, mut consumer) = buffer.split();

        assert_eq!(producer.write(&[1, 2, 3]), 3);
        assert_eq!(consumer.available(), 3);
        assert_eq!(producer.free(), 5);

        let mut output = [0u8; 8];
        assert_eq!(consumer.read(&mut output), 3);
        assert_eq!(output[..3], [1, 2, 3]);
        assert_eq!(consumer.available(), 0);
    }

    #[test]
    fn partial_write_when_full() {
        let mut buffer = RingBuffer::<4>::new();
        let (mut producer, _consumer) = buffer.split();

        assert_eq!(producer.write(&[1, 2, 3, 4, 5, 6]), 4);
        assert_eq!(producer.write(&[7]), 0);
        assert_eq!(producer.free(), 0);
    }

    #[test]
    fn wraps_around() {
        let mut buffer = RingBuffer::<5>::new();
        let (mut producer, mut consumer) = buffer.split();
        let mut output = [0u8; 5];

        for round in 0..20u8 {
            let data = [round, round.wrapping_add(1), round.wrapping_add(2)];

            assert_eq!(producer.write(&data), 3);
            assert_eq!(consumer.read(&mut output), 3);
            assert_eq!(output[..3], data);
        }
    }

    #[test]
    fn grants_are_contiguous() {
        let mut buffer = RingBuffer::<8>::new();
        let (mut producer, mut consumer) = buffer.split();

        producer.write(&[0; 6]);
        consumer.release(6);

        assert_eq!(producer.grant().len(), 2);
        producer.grant().copy_from_slice(&[1, 2]);
        producer.commit(2);
        assert_eq!(producer.grant().len(), 6);
        producer.grant()[..2].copy_from_slice(&[3, 4]);
        producer.commit(2);

        assert_eq!(consumer.readable(), [1, 2]);
        consumer.release(2);
        assert_eq!(consumer.readable(), [3, 4]);
    }

    #[test]
    fn commit_and_release_are_bounded() {
        let mut buffer = RingBuffer::<4>::new();
        let (mut producer, mut consumer) = buffer.split();

        producer.commit(100);
        assert_eq!(consumer.available(), 4);

        consumer.release(100);
        assert_eq!(consumer.available(), 0);
    }

    #[test]
    fn clear_drops_pending_data() {
        let mut buffer = RingBuffer::<4>::new();
        let (mut producer, mut consumer) = buffer.split();

        producer.write(&[1, 2, 3]);
        consumer.clear();

        assert_eq!(consumer.available(), 0);
        assert_eq!(producer.free(), 4);
    }

    #[test]
    #[cfg_attr(
        all(target_family = "wasm", not(target_feature = "atomics")),
        ignore = "threads are not available"
    )]
    fn concurrent_transfer() {
        let mut buffer = RingBuffer::<7>::new();
        let (mut producer, mut consumer) = buffer.split();
        const TOTAL: usize = if cfg!(miri) { 2_000 } else { 100_000 };

        std::thread::scope(|scope| {
            scope.spawn(move || {
                let mut next = 0usize;

                while next < TOTAL {
                    let end = (next + 3).min(TOTAL);
                    let data: std::vec::Vec<u8> = (next..end).map(|value| value as u8).collect();
                    next += producer.write(&data);
                }
            });

            let mut expected = 0usize;
            let mut output = [0u8; 5];

            while expected < TOTAL {
                let count = consumer.read(&mut output);

                for value in &output[..count] {
                    assert_eq!(*value, expected as u8);
                    expected += 1;
                }
            }
        });
    }
}
