# Contributing

To contribute to the book, you wil need `mdbook`, `mdbook-mermaid`, and `mdbook-linkcheck` installed.
These can be installed through `cargo install` or through your package manager:

```sh
cargo install mdbook mdbook-mermaid mdbook-linkcheck
```

They are also available as packages in Nix:

```sh
nix-shell -p mdbook mdbook-mermaid mdbook-linkcheck
```

You will need to install `mdbook-mermaid`'s CSS/JS into the book's `book` directory:

```sh
mdbook-mermaid install book
```

You can now serve the book locally:

```sh
mdbook serve book
```

For more information, please consult the [mdBook documentation](https://rust-lang.github.io/mdBook/).