# Connecting to a remote server

You've tested your multiplayer locally, and now you want to try connecting to a remote server.


This is a quick guide mostly for myself, as I knew nothing about networking and didn't know where to start.


## Set up a server

From what I understand you can either:
- rent a dedicated node with a cloud provider. Smaller cloud providers (Linode, DigitalOcean) seem cheaper than bigger ones (Google Cloud, AWS) unless you have free credits.
  I used Kamatera, which worked excellently
- rent a VPS, which is a virtual machine on a dedicated node. I believe this means that you are sharing the resources of the nodes with other customers.

You need to get the public ip address of your server: S-IP.


## Connect using UDP

On the client:
- You will need to create a local UDP socket that binds your **local client ip address**
  - I believe that when the server receives a packet on their UDP socket, they will receive the **public ip address** of the client.
    By specifying the local client ip address, you enable the server to do NAT-traversal? I'm not sure.
  - You can either manually specify your local client ip address, or you can use `0.0.0.0` (INADDR_ANY), which binds
    to any of the local ip addresses of your machine. I think that if your computer only has 1 network card, then it will
    bind to that one. (source: [Beej's networking guide](https://beej.us/guide/bgnet/html/index-wide.html#bindman))


On the server:
- Same thing, you will need to create UDP socket that binds to your **local server ip address**.
  - Again, you can either manually specify your local server ip address, or you can use `0.0.0.0` (INADDR_ANY) if your machine
    only has 1 network card, or if you don't care which of the local ip addresses the socket binds to
  - Keep track of the port used for the server: S-P
- You will need to keep track of the public ip address of your server: S-IP
- Start the lightyear process on the server, for example with `cargo run --example interest_management -- server --headless`

On the client:
- connect to the server by specifying the server public ip address and the server port:
`cargo run --example interest_management -- client -c 1 --server-addr=S-IP --server-port=S-P`