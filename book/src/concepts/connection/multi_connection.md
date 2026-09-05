# Multi connection

In lightyear, connections are just entities with IO components, so a server can serve several transports at the same time.
This means that the server could:
- open a port to establish steam socket connections
- open another port for UDP connections
- open another port for WebTransport connections
- etc.

and have all these connections running at the same time.

You can therefore have cross-play between different platforms.

Another potential usage is to have a "HostServer" setup where a client acts as the "host":
- the Client and the Server run in the same process (this is the `HostClient` topology; the `simple_box` example runs it with `Mode::HostClient`)
- the in-process client talks to the server over local (crossbeam) channels
- other clients can still connect to the same server over UDP, WebTransport, etc.
