//! A broadcast medium, in process: every node attached hears every frame
//! another node sends, and none hears its own.
//!
//! A CAN bus is one, so is an Ethernet segment and so is the air a radio
//! protocol speaks over. What a protocol on such a medium is proved against:
//!
//! - **Every other node hears it.** A frame goes to every node but the one
//!   that sent it; each node reads its own arrivals, in order, so two
//!   listeners never compete for one frame.
//! - **Nobody there is an error.** A frame no other node is attached to hear
//!   is not acknowledged, and the sender is told so, as a CAN controller is.
//! - **Frames are lost on demand**: every `n`th frame the medium carries, for
//!   the retries a protocol is supposed to survive.
//!
//! The medium is generic over the frame: a technology keeps its own frame and
//! its own trait for the medium, and maps a [`Node`] onto it. Until 2026-09-24
//! the CAN riders faked one bus with two directed queues per exchange, because
//! the in-process bus they had returned a node's own frames to it and let two
//! readers race for one (ADR-0061, simulators in the SDK).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use transport::error::{Result, TransportError};

/// How often a waiting reader looks again.
const POLL: Duration = Duration::from_millis(1);

/// The medium's shared state: each node's arrivals, and what it is told to
/// lose.
struct Air<F> {
    inboxes: Vec<VecDeque<F>>,
    lose_every: Option<u32>,
    carried: u32,
}

/// A broadcast medium carrying frames of type `F`.
pub struct Medium<F> {
    name: String,
    air: Arc<Mutex<Air<F>>>,
}

/// One node on a [`Medium`]: it sends to every other node and reads what
/// they send it.
pub struct Node<F> {
    name: String,
    index: usize,
    air: Arc<Mutex<Air<F>>>,
}

impl<F: Clone + Send> Medium<F> {
    /// An empty medium called `name`, which origin URIs carry.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            air: Arc::new(Mutex::new(Air {
                inboxes: Vec::new(),
                lose_every: None,
                carried: 0,
            })),
        }
    }

    /// A new node on the medium, hearing what is sent from now on.
    #[must_use]
    pub fn node(&self) -> Node<F> {
        let mut air = lock(&self.air);
        air.inboxes.push(VecDeque::new());
        Node {
            name: self.name.clone(),
            index: air.inboxes.len() - 1,
            air: Arc::clone(&self.air),
        }
    }

    /// Lose every `every`th frame the medium carries: sent, acknowledged by
    /// nobody, heard by nobody.
    pub fn lose_every(&self, every: u32) {
        lock(&self.air).lose_every = Some(every.max(1));
    }
}

impl<F: Clone + Send> Node<F> {
    /// The medium's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Send `frame` to every other node on the medium.
    ///
    /// # Errors
    /// No other node is attached to acknowledge it, or the medium lost it —
    /// both retryable, as a CAN controller retries an unacknowledged frame.
    pub fn transmit(&self, frame: &F) -> Result<()> {
        let mut air = lock(&self.air);
        air.carried += 1;
        let carried = air.carried;
        if air
            .lose_every
            .is_some_and(|every| carried.is_multiple_of(every))
        {
            return Err(TransportError::retryable(
                "the frame was lost on the medium",
            ));
        }
        let mut heard = false;
        for (index, inbox) in air.inboxes.iter_mut().enumerate() {
            if index != self.index {
                inbox.push_back(frame.clone());
                heard = true;
            }
        }
        if heard {
            Ok(())
        } else {
            Err(TransportError::retryable(
                "no other node acknowledged the frame",
            ))
        }
    }

    /// The next frame another node sent this one, or `None` when none came
    /// within `timeout`.
    ///
    /// # Errors
    /// Never on this medium; the signature is a medium's.
    pub fn receive(&self, timeout: Duration) -> Result<Option<F>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(frame) = lock(&self.air).inboxes[self.index].pop_front() {
                return Ok(Some(frame));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_other_node_hears_a_frame_and_the_sender_does_not() {
        let medium = Medium::new("can0");
        let (a, b, c) = (medium.node(), medium.node(), medium.node());
        a.transmit(&1u8).expect("heard");
        assert_eq!(
            a.receive(Duration::ZERO).expect("read"),
            None,
            "not its own"
        );
        assert_eq!(b.receive(Duration::ZERO).expect("read"), Some(1));
        assert_eq!(c.receive(Duration::ZERO).expect("read"), Some(1), "no race");
        assert_eq!(b.name(), "can0");
    }

    #[test]
    fn a_frame_nobody_hears_is_not_acknowledged() {
        let medium = Medium::new("can0");
        let alone = medium.node();
        let refused = alone.transmit(&7u8).expect_err("nobody there");
        assert!(refused.retryable, "{refused}");
        let other = medium.node();
        alone.transmit(&7u8).expect("heard now");
        assert_eq!(other.receive(Duration::ZERO).expect("read"), Some(7));
    }

    #[test]
    fn a_lost_frame_is_heard_by_nobody_and_the_sender_is_told() {
        let medium = Medium::new("air");
        let (a, b) = (medium.node(), medium.node());
        medium.lose_every(2);
        a.transmit(&1u8).expect("first carried");
        assert!(a.transmit(&2u8).expect_err("second lost").retryable);
        a.transmit(&3u8).expect("third carried");
        assert_eq!(b.receive(Duration::ZERO).expect("read"), Some(1));
        assert_eq!(b.receive(Duration::ZERO).expect("read"), Some(3));
        assert_eq!(b.receive(Duration::ZERO).expect("read"), None);
    }

    #[test]
    fn a_reader_waits_for_a_frame_sent_from_another_thread() {
        let medium = Medium::new("can0");
        let (a, b) = (medium.node(), medium.node());
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            a.transmit(&9u8)
        });
        assert_eq!(b.receive(Duration::from_secs(2)).expect("waited"), Some(9));
        sender.join().expect("joined").expect("sent");
    }
}
