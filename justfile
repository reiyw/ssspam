lint:
  cargo +nightly clippy -- -D clippy::all -W clippy::nursery
  cargo +nightly fmt -- --check

fix:
  cargo +nightly clippy --fix --allow-dirty -- -D clippy::all -W clippy::nursery
  cargo +nightly fmt
