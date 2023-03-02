#![allow(unused)]

use std::{
    collections::VecDeque,
    mem,
    ops::DerefMut,
    sync::{Arc, Condvar, Mutex},
    thread,
};

struct Inner<T> {
    mu: Mutex<Critical<T>>,
    cond: Condvar,
}

struct Critical<T> {
    buf: VecDeque<T>,
    senders: usize,
    done: bool,
}

pub struct SendErr<T>(pub T);

pub struct Sender<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Sender<T> {
    fn send(&self, val: T) -> Result<(), T> {
        // acquire mutex, add a value to the send queue and signal to potential recievers waiting.
        let mut guard = self.inner.mu.lock().unwrap();
        if guard.done {
            return Err(val);
        }
        guard.buf.push_front(val);
        drop(guard); // drop guard since we need the reciever to be able to acquire it after the
                     // signal.
        self.inner.cond.notify_one(); // notify the only one possible listener.
        Ok(())
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut guard = self.inner.mu.lock().unwrap();
        if guard.done {
            // reciever exited.
            return;
        }
        guard.senders -= 1;
        if guard.senders == 0 {
            guard.done = true;
            drop(guard);
            self.inner.cond.notify_one(); // notify possibly hanging reciever.
        }
    }
}

pub struct Reciever<T> {
    inner: Arc<Inner<T>>,
    local_buf: VecDeque<T>,
}

impl<T> Reciever<T> {
    fn recv(&mut self) -> Option<T> {
        // try to consume local copy of buffer first to reduce mutex contention.
        if let Some(v) = self.local_buf.pop_back() {
            return Some(v);
        }

        // go in a cycle of acquiring mutex -> check if we have any work to do -> go back to sleep
        // (loop also mostly accounts for spureous wake ups)
        loop {
            let mut guard = self.inner.mu.lock().unwrap();
            match guard.buf.pop_back() {
                // message on queue, recieve it and return it.
                Some(v) => {
                    // swap our local (empty) buffer (which has an allocated capacity) with the
                    // incoming buffer.
                    //
                    // this will keep some data local taking advantage that we only have one
                    // reciever, this data we can acess without interacting with the mutex.
                    mem::swap(&mut self.local_buf, &mut guard.buf);
                    return Some(v);
                }
                // we got woken up because all workers got dropped.
                None if guard.done => return None,
                // spureous wakeup or first call to an empty buffer. Anyways we go back to sleep
                // untill something "interesting" happens (one of the above).
                None => {
                    self.inner.cond.wait(guard);
                }
            }
        }
    }
}

impl<T> Drop for Reciever<T> {
    fn drop(&mut self) {
        // set done to true to stop senders from blocking.
        let mut guard = self.inner.mu.lock().unwrap();
        guard.done = true;
    }
}

pub fn unbounded<T>() -> (Sender<T>, Reciever<T>) {
    // these 2 types, sender and reciever need to both share some memory
    // and logic to report back to:
    //      - When a read occurs.
    //      - When a write occurs.
    //      - When a Sender / Reciever gets dropped.

    let inner = Inner {
        mu: Mutex::new(Critical {
            buf: VecDeque::default(),
            senders: 1,
            done: false,
        }),
        cond: Condvar::default(),
    };
    let inner = Arc::new(inner);
    let rx = Reciever {
        inner: Arc::clone(&inner),
        local_buf: VecDeque::default(),
    };
    let tx = Sender {
        inner: Arc::clone(&inner),
    };

    (tx, rx)
}

#[test]
fn test_interface() {
    println!("Hello");
    let (tx, mut rx) = unbounded();
    let handle = thread::spawn(move || {
        // block on recieve:
        let val = rx.recv().unwrap();
        println!("{}", val);
    });

    tx.send(5);
    handle.join();
}

#[test]
fn ping_pong() {
    let (mut tx, mut rx) = unbounded();
    tx.send(42);
    assert_eq!(rx.recv(), Some(42));
}

#[test]
fn closed_tx() {
    let (tx, mut rx) = unbounded::<()>();
    drop(tx);
    assert_eq!(rx.recv(), None);
}

#[test]
fn closed_rx() {
    let (mut tx, rx) = unbounded();
    drop(rx);
    tx.send(42);
}

// uncomment if you want to check for blocking behaviour.
// #[test]
// fn test_blocking() {
//     println!("Hello");
//     let (tx, mut rx) = unbounded();
//     let handle = thread::spawn(move || {
//         // block on recieve:
//         let val = rx.recv().unwrap();
//         println!("{}", val);
//         let val = rx.recv().unwrap();
//         println!("{}", val);
//     });
//
//     tx.send(5);
//     handle.join();
// }
