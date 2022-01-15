use std::io::Write;
use std::path::PathBuf;
use std::{fs::File, path::Path};

use chrono::{DateTime, Utc};
use dotenv::dotenv;
use maud::{html, PreEscaped, DOCTYPE};
use serde::Serialize;
use structopt::StructOpt;

use ssspambot::{load_sounds_try_from_cache, SoundDetail};

#[derive(Debug, StructOpt)]
#[structopt(name = "gen_viewer")]
struct Opt {
    #[structopt(long, parse(from_os_str), env)]
    sound_dir: PathBuf,

    #[structopt(long, parse(from_os_str))]
    dest: PathBuf,
}

fn main() -> anyhow::Result<()> {
    dotenv().ok();
    let opt = Opt::from_args();

    let markup = html! {
        html {
            (DOCTYPE)
            head {
                meta charset="utf-8" {}
                meta name="viewport" content="width=device-width, initial-scale=1" {}
                title { "surfpvparena sounds" }

                link rel="stylesheet" type="text/css" href="https://cdnjs.cloudflare.com/ajax/libs/bulma/0.9.2/css/bulma.min.css" {}
                link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/bulma-slider@2.0.4/dist/css/bulma-slider.min.css" {}
                script src="https://cdn.jsdelivr.net/npm/bulma-slider@2.0.4/dist/js/bulma-slider.min.js" {}

                link rel="stylesheet" type="text/css" href="https://cdn.datatables.net/v/bm/jq-3.6.0/dt-1.11.3/datatables.min.css" {}
                script type="text/javascript" src="https://cdn.datatables.net/v/bm/jq-3.6.0/dt-1.11.3/datatables.min.js" {}

                script { (PreEscaped(r#"
                function setVolume() {
                    const volume = document.getElementById('volume').value;
                    const audio_elems = document.getElementsByTagName('audio');
                    for (const audio of audio_elems) {
                        audio.volume = volume;
                    }
                }

                $(document).ready(function () {
                    $('#sounds').DataTable({
                        'ajax': 'data.json',
                        'deferRender': true,
                        'columnDefs': [{
                            'targets': 3,
                            'render': function(data, type, row, meta) {
                                return '<audio controls="controls" preload="none" src="' + data + '"></audio>';
                            },
                        }]
                    });
                    setVolume();

                    $('#sounds').on('DOMSubtreeModified', function () {
                        setVolume();
                    });
                });
                "# ))}
            }
            body {
                section class="section" {
                div class = "container" {
                div class="columns" {
                    label {
                        "Volume:"
                        input id="volume" class="slider is-fullwidth" min="0" max="1" value="0.1" step="0.05" type="range" onchange="setVolume()" {}
                    }
                }
                table id="sounds" class="table is-striped is-fullwidth" {
                    thead {
                        tr {
                            th { "Name" }
                            th { "Duration" }
                            th { "Updated" }
                            th { "Player" }
                        }
                    }
                    tbody {}
                }
                }
                }
            }
        }
    };

    let mut f = File::create(&opt.dest.join("index.html"))?;
    f.write_all(markup.into_string().as_bytes())?;

    gen_data(&opt.sound_dir, &opt.dest.join("data.json"))?;

    Ok(())
}

#[derive(Debug, Serialize)]
struct Data {
    data: Vec<(String, String, String, String)>,
}

fn gen_data(sound_dir: impl AsRef<Path>, out_file: impl AsRef<Path>) -> anyhow::Result<()> {
    let sounds: Vec<SoundDetail> = load_sounds_try_from_cache(sound_dir)
        .values()
        .cloned()
        .collect();
    let mut data: Vec<(String, String, String, String)> = Vec::new();
    // for sound in sounds[..100].iter().cloned() {
    for sound in sounds {
        let updated_at: DateTime<Utc> = sound.updated_at.into();
        let src = format!(
            "sound/{}",
            sound.path.file_name().unwrap().to_str().unwrap()
        );
        let row = (
            sound.name,
            format!("{:.1}", sound.duration.as_secs_f64()),
            updated_at.format("%Y-%m-%d").to_string(),
            src,
        );
        data.push(row);
    }
    let data = Data { data };
    let j = serde_json::to_string(&data)?;
    let mut f = File::create(out_file)?;
    f.write_all(j.as_bytes())?;
    Ok(())
}
