use std::{fs::File, io::Write, path::PathBuf};

use clap::Parser;
use dotenvy::dotenv;
use maud::{html, PreEscaped, DOCTYPE};
use ssspambot::web::gen_data_json_from_sound_dir;

#[derive(Parser)]
#[clap(version, about)]
struct Opt {
    #[clap(long, env, value_parser)]
    sound_dir: PathBuf,

    #[clap(long, value_parser)]
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

                link rel="stylesheet" type="text/css" href="DataTables-1.11.3/css/dataTables.bulma.min.css" {}

                script type="text/javascript" src="jQuery-3.6.0/jquery-3.6.0.min.js" {}
                script type="text/javascript" src="DataTables-1.11.3/js/jquery.dataTables.min.js" {}
                script type="text/javascript" src="DataTables-1.11.3/js/dataTables.bulma.js" {}

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

    let mut f = File::create(opt.dest.join("index.html"))?;
    f.write_all(markup.into_string().as_bytes())?;

    gen_data_json_from_sound_dir(&opt.sound_dir, opt.dest.join("data.json"))?;

    Ok(())
}
