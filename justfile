lint:
  cargo +nightly clippy -- -D clippy::all -W clippy::nursery
  cargo +nightly fmt -- --check
  hadolint --ignore DL3059 --ignore DL3008 Dockerfile

fix:
  cargo +nightly clippy --fix -- -D clippy::all -W clippy::nursery
  cargo +nightly fmt
