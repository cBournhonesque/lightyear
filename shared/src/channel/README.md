Senders: 
- can be reliable (by keeping track of the messages that were not acked in time)
- or unreliable (just send the messages and forget about it)
  - sequenced: send the message id
  - unordered: don't even include the message id


Receivers:
- reliable: make sure we receive every single message. That means we must receive message 0, 1, 2, ... etc. We buffer all
  messages received that are more recent than the next one we need
  - ordered: the next one we need progresses sequentially (1, 2, 3). We return messages in order 
  - sequenced: // TODO? the next one we need progresses sequentially, unless we receive a more recent message; then we start from there 
  - unordered: the next one we need progresses sequentially. We return messages from the buffer in any order.
- unreliable:
  - sequenced: just receive the messages, but ignore ones that are older than the most recent message
  - unordered: just receive the messages