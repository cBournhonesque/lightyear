# Introduction

For a quick tutorial about how to use the crate, click [here](./tutorial/title.md).

## What is lightyear?

Lightyear is a networking library for games written in Bevy.
It uses a client-server networking architecture, where the server is authoritative over the game state.

It is heavily inspired by [naia](https://github.com/naia-lib/naia).


## What is this book about?

This book serves several purposes:
- It contains some explanations of game networking concepts, as well as how they are implemented in this crate
- provide some examples of how to use the crate
- explain some of the design decisions that were made

This book does not aim to be a polished document, or a comprehensive reference for lightyear.
It is more of a collection of notes and thoughts that I had while developing the crate. Plus I wanted to have some kind of "wiki"
I could come back to later to remember why I did things a certain way.


## Who am I?

I don't have a lot of experience in Rust, and have never worked on game development.
I picked up Bevy a couple years ago and got really interested in game-dev, specifically in networking.
I've started reading more networking resources such as [GafferOnGames](https://gafferongames.com/) or the Overwatch and RocketLeague GDC talks.

I decided to write this crate to help me get better at Rust, get hands-on knowledge of networking for games, and of course
to provide a useful library for the community.