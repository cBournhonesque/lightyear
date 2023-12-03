# Vendored crates

Crates.io does not allow publishing crates that do not have explicit versions (for example git branches),
so I'm vendoring some dependencies here.

## bitcode

```toml
bitcode = { git = "https://github.com/cBournhonesque/bitcode.git", branch = "cb/latest", features = [
  "serde",
] }
```

Had to update the `WordWriter` trait slightly, in particular some of the lifetimes.
Waiting for the next version of bitcode to be released.


## wtransport

```toml
wtransport = { git = "https://github.com/BiagioFesta/wtransport.git", branch = "master", optional = true }
```

Waiting for the next version of wtransport to be released.
(Contained some fixes for the `Certificate`)