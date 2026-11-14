# yoink web

The browser app lives here: a `wasm-bindgen` wrapper around the shared
`../core` crate, plus the static site in `../web/www`.

```sh
sh build.sh   # writes www/pkg, needs wasm32-unknown-unknown + wasm-bindgen-cli
```

`.github/workflows/pages.yml` deploys `www` to GitHub Pages on every push to
this branch; `.github/workflows/ci.yml` runs the core tests and clippy.

This branch is an independent snapshot of the repo. The CLI lives on the `cli`
branch, so changes to `../core` must be applied there too.
