//! Handles storing the state of the connection over time for temporal checking of the stream.
use tungstenite::Message;

#[derive(Debug)]
struct StoredMessage {
    /// Index in the message stream
    index: usize,
    /// The message contents
    message: Message,
    /// The number of matchers interested
    count: usize,
}

/// Mutable state shared for every matcher. For more thorough documentation [`Match`](crate::Match) can be
/// consulted.
#[derive(Debug)]
pub struct MatchState {
    messages_received: usize,
    stored_messages: Vec<StoredMessage>,
}

impl MatchState {
    #[doc(hidden)]
    pub fn new() -> Self {
        MatchState {
            messages_received: 0,
            stored_messages: vec![],
        }
    }

    pub(crate) fn push_message(&mut self, message: Message) {
        self.stored_messages.push(StoredMessage {
            index: self.messages_received,
            message,
            count: 0,
        });
        self.messages_received += 1;
    }

    /// Specify that you want the message to be kept in the history for temporal checking.
    ///
    /// # Panics
    ///
    /// This method will panic if the given message index is not present.
    ///
    /// # Example
    ///
    /// ```
    /// use wiremocket::prelude::*;
    ///
    /// fn check_state(state: &mut MatchState) {
    ///     // There has been 1 message in the state, this will be the first message and isn't yet
    ///     // set to be remembered or forgotten
    ///     assert_eq!(state.len(), 1);
    ///
    ///     assert_eq!(state.last(), state.get_message(0).unwrap());
    ///     state.keep_message(0)
    /// }
    ///
    /// # let mut state = default_doctest_state();
    /// # check_state(&mut state);
    /// ```
    ///
    /// ```should_panic
    /// use wiremocket::prelude::*;
    ///
    /// fn check_state(state: &mut MatchState) {
    ///     // There has been 1 message in the state, this will be the first message and isn't yet
    ///     // set to be remembered or forgotten.
    ///     assert_eq!(state.len(), 1);
    ///
    ///     // Out of bounds but something evicted but in bounds will also panic.
    ///     state.keep_message(1)
    /// }
    ///
    /// # let mut state = default_doctest_state();
    /// # check_state(&mut state);
    /// ```
    ///
    pub fn keep_message(&mut self, index: usize) {
        if let Some(stored) = self
            .stored_messages
            .iter_mut()
            .rev()
            .find(|x| x.index == index)
        {
            stored.count += 1;
        } else {
            panic!("Message index not found");
        }
    }

    pub(crate) fn evict(&mut self) {
        self.stored_messages.retain(|x| x.count > 0)
    }

    /// Forgets a message from the state. Unlike [`MatchState::keep_message`] this won't panic if
    /// the message isn't found. If we don't want to remember it then it not being present isn't a
    /// fail-state.
    pub fn forget_message(&mut self, index: usize) {
        if let Some(stored) = self
            .stored_messages
            .iter_mut()
            .rev()
            .find(|x| x.index == index)
        {
            stored.count = stored.count.saturating_sub(1);
        }
    }

    /// Gets the last message that arrived into the system.
    pub fn last(&self) -> &Message {
        &self.stored_messages.last().unwrap().message
    }

    /// Gets a message at a specific index - or `None` if the message was forgotten.
    pub fn get_message(&self, index: usize) -> Option<&Message> {
        self.stored_messages
            .iter()
            .find(|x| x.index == index)
            .map(|x| &x.message)
    }

    /// Get the number of messages received by the server for the active connection.
    pub fn len(&self) -> usize {
        self.messages_received
    }
}

#[doc(hidden)]
pub fn default_doctest_state() -> MatchState {
    let mut state = MatchState::new();
    state.push_message(Message::Close(None));
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_eviction() {
        let mut state = MatchState::new();

        state.push_message(Message::text("hello"));
        assert_eq!(state.stored_messages.len(), 1);
        assert_eq!(state.messages_received, 1);

        state.evict();
        assert_eq!(state.stored_messages.len(), 0);
        assert_eq!(state.messages_received, 1);

        state.push_message(Message::text("hello"));
        state.keep_message(1);
        state.evict();
        assert_eq!(state.stored_messages.len(), 1);
        assert_eq!(state.messages_received, 2);

        state.forget_message(0);
        state.evict();
        assert_eq!(state.stored_messages.len(), 1);

        state.forget_message(1);
        assert_eq!(state.stored_messages.len(), 1);
        state.evict();
        assert_eq!(state.stored_messages.len(), 0);
    }
}
