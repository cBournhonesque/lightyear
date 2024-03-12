# Reliability

In this layer we add some mechanisms to be able to send and receive messages reliably or in a given order.

It is similar to the [reliable](https://github.com/networkprotocol/reliable) layer created by Glenn Fiedler on top of his netcode.io code.

This layer introduces:
- reliability: make sure a packets is received by the remote peer
- ordering: make sure packets are received in the same order they were sent
- channels: allow to send packets on different channels, which can have different reliability and ordering guarantees

