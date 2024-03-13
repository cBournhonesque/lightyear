# Multi connection


In lightyear, the server can handle multiple connection protocol as the same time.
This means that the server could:
- open a port to establish steam socket connections
- open another port for UDP connections
- open another port for WebTransport connections
- etc.

and have all these connections running at the same time.

You can therefore have cross-play between different platforms.

Another potential usage is to have a "ListenServer" setup where a client acts as the "host":
- the Client and the Server run in the same process
- the Server has multiple connection protocols:
  - one is based on local channels to talk to the Client that is running in the same process
  - the other could be for example a UDP connection to allow other clients to connect to the server


To achieve this, you can just provide multiple `NetConfig` when creating the `ServerConfig` that will be used to create the server.