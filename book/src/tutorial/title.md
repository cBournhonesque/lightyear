# Tutorial

This section will teach you how to quickly setup networking in your bevy game using this crate.

You can find an example game in the [examples](https://github.com/cBournhonesque/lightyear/tree/main/examples) folder.

In this tutorial, we will reproduce
the [simple box example](https://github.com/cBournhonesque/lightyear/tree/main/examples/simple_box) to demonstrate the
features of this crate.

We will build a simple game where each client can move a "box" on the screen, the box will be replicated to the server
and to all other clients.