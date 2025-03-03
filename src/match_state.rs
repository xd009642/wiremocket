use tungstenite::Message;

#[derive(Debug)]
pub struct StoredMessage {
    /// Index in the message stream
    index: usize,
    /// The message contents
    message: Message,
    /// The number of matchers interested
    count: usize,
}

/// An ideal for temporal matching of requests. We might only want to keep some messages and not
/// others
#[derive(Debug, Default)]
pub struct MatchState {
    messages_received: usize,
    stored_messages: Vec<StoredMessage>,
}

impl MatchState {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn push_message(&mut self, message: Message) {
        self.stored_messages.push(StoredMessage {
            index: self.messages_received,
            message,
            count: 0,
        });
        self.messages_received += 1;
    }

    pub fn keep_message(&mut self, index: usize) {
        if let Some(stored) = self
            .stored_messages
            .iter_mut()
            .rev()
            .find(|x| x.index == index)
        {
            stored.count += 1;
        }
    }

    pub fn evict(&mut self) {
        self.stored_messages.retain(|x| x.count > 0)
    }

    pub fn forget_message(&mut self, index: usize) {
        if let Some(stored) = self
            .stored_messages
            .iter_mut()
            .rev()
            .find(|x| x.index == index)
        {
            stored.count.saturating_sub(1);
        }
    }

    pub fn last_unchecked(&mut self) -> &Message {
        &self.stored_messages.last().unwrap().message
    }

    pub fn get_message(&self, index: usize) -> Option<&Message> {
        self.stored_messages
            .iter()
            .find(|x| x.index == index)
            .map(|x| &x.message)
    }

    pub fn len(&self) -> usize {
        self.messages_received
    }
}
