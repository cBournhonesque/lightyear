# Packet


On top of the transport layer (which lets us send some arbitrary bytes) we have the packet layer.

A packet is a structure that contains some data and some metadata.
The data will be a list of Messages that are contained in the packet.

A message is a structure that knows how to serialize/deserialize itself.


This is how we store messages into packets:
- the message get serialized into raw bytes
- if the message is over the packet limit size (roughly 1200 bytes), it gets fragmented into multiple parts
- we build a packet by iterating through the channels in order of priority, and then storing as many messages we can into the packet

