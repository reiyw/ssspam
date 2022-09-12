set windows-shell := ["pwsh.exe", "-NoLogo", "-Command"]

build-pi $TARGET_AR="arm-none-linux-gnueabihf-ar.exe" $TARGET_CC="arm-none-linux-gnueabihf-gcc.exe" $LIBOPUS_NO_PKG="":
  cargo build --target armv7-unknown-linux-musleabihf --release -v

deploy: build-pi
  scp target/armv7-unknown-linux-musleabihf/release/ssspambot pi:/home/pi/local/bin/

lint:
  cargo clippy -- -D clippy::all -W clippy::nursery
  cargo +nightly fmt -- --check
  hadolint --ignore DL3059 --ignore DL3008 Dockerfile

update-server:
  gcloud beta compute instance-groups managed rolling-action replace instance-group-1 --max-surge=1 --max-unavailable=0 --min-ready=30s --replacement-method=substitute --zone=asia-northeast1-b

update-sounds today:
  gsutil -m rsync gs://surfpvparena/dist/sound dist/sound

  find dist/sound -name '*.mp3' | xargs zip -j {{today}}.zip
  gsutil cp {{today}}.zip gs://surfpvparena/{{today}}.zip
  rm {{today}}.zip

  cargo run --release --bin gen_viewer -- --dest dist --sound-dir dist/sound
  gsutil cp dist/data.json gs://surfpvparena/dist/data.json
