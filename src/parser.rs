// TODO: rename this file

use std::str::FromStr;

use nom::{
    branch::alt,
    bytes::complete::{tag, take_while1},
    character::complete::{char, multispace0, u32},
    combinator::{eof, map, opt},
    error::{Error, ParseError},
    multi::{many0, many1},
    number::complete::double,
    sequence::{delimited, pair, preceded, terminated},
    Finish, IResult,
};

#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord, Hash)]
pub enum Action {
    Synthesize,
    Concat,
}

#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord, Hash, Builder)]
#[builder(default)]
pub struct SayCommand {
    pub name: String,
    pub speed: u32,
    pub pitch: u32,
    pub wait: u32,
    pub start: u32,
    pub duration: Option<u32>,
    pub stop: bool,
    pub action: Action,
}

impl Default for SayCommand {
    fn default() -> Self {
        Self {
            name: "".into(),
            speed: 100,
            pitch: 100,
            wait: 100,
            start: 0,
            duration: None,
            stop: false,
            action: Action::Synthesize,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Command {
    Say(SayCommand),
    Wait(u32),
}

impl ToString for Command {
    fn to_string(&self) -> String {
        match self {
            Self::Say(cmd) => {
                let mut s = cmd.name.clone();
                if cmd.speed != 100 {
                    s += &format!(" {}", cmd.speed);
                }
                if cmd.pitch != 100 {
                    s += &format!(" p{}", cmd.pitch);
                }
                if cmd.wait != 100 {
                    s += &format!(" w{:.1}", (cmd.wait as f64) / 1000.0);
                }
                if cmd.start != 0 {
                    s += &format!(" s{:.1}", (cmd.start as f64) / 1000.0);
                }
                if let Some(dur) = cmd.duration {
                    s += &format!(" d{:.1}", (dur as f64) / 1000.0);
                }
                if cmd.stop {
                    s += " s";
                }
                match cmd.action {
                    Action::Synthesize => s += "; ",
                    Action::Concat => s += "| ",
                }
                s
            }
            Self::Wait(n) => format!("~w{:.1}; ", (*n as f64) / 1000.0),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Commands(Vec<Command>);

impl Commands {
    pub fn iter(&self) -> std::slice::Iter<'_, Command> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn sanitize(&mut self) {
        let mut cmds = Vec::new();
        let mut consecutive_immediate_sounds = 1;
        for cmd in self.0.iter_mut() {
            match cmd {
                Command::Say(cmd) => {
                    cmd.pitch = std::cmp::max(cmd.pitch, 1);
                    cmd.pitch = std::cmp::min(cmd.pitch, 200);

                    if consecutive_immediate_sounds > 2 {
                        continue;
                    }

                    if cmd.wait < 100 {
                        consecutive_immediate_sounds += 1
                    } else {
                        consecutive_immediate_sounds = 1;
                    }
                }
                Command::Wait(_) => (),
            }
            cmds.push(cmd.clone());
        }
        self.0 = cmds;
    }
}

impl IntoIterator for Commands {
    type Item = Command;

    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl FromStr for Commands {
    type Err = Error<String>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match commands(s).finish() {
            Ok((_remaining, commands)) => Ok(Self(commands)),
            Err(Error { input, code }) => Err(Error {
                input: input.to_string(),
                code,
            }),
        }
    }
}

impl ToString for Commands {
    fn to_string(&self) -> String {
        let mut s: String = self.iter().map(|c| c.to_string()).collect();
        s.truncate(s.len() - 2);
        s
    }
}

enum SayArg {
    Speed(u32),
    Pitch(u32),
    Wait(u32),
    Start(u32),
    Duration(u32),
    Stop,
}

fn speed(i: &str) -> IResult<&str, u32> {
    ws(preceded(opt(char('@')), u32))(i)
}

fn pitch(i: &str) -> IResult<&str, u32> {
    ws(preceded(char('p'), u32))(i)
}

fn wait(i: &str) -> IResult<&str, f64> {
    ws(preceded(char('w'), double))(i)
}

fn start(i: &str) -> IResult<&str, f64> {
    ws(preceded(char('s'), double))(i)
}

fn duration(i: &str) -> IResult<&str, f64> {
    ws(preceded(char('d'), double))(i)
}

fn stop(i: &str) -> IResult<&str, &str> {
    ws(alt((tag("stop"), tag("s"))))(i)
}

fn action(i: &str) -> IResult<&str, &str> {
    ws(alt((tag(";"), tag("|"), eof)))(i)
}

fn say_arg(input: &str) -> IResult<&str, SayArg> {
    alt((
        map(speed, SayArg::Speed),
        map(pitch, SayArg::Pitch),
        map(wait, |n| SayArg::Wait((n * 1000.0) as u32)),
        map(start, |n| SayArg::Start((n * 1000.0) as u32)),
        map(duration, |n| SayArg::Duration((n * 1000.0) as u32)),
        map(stop, |_| SayArg::Stop),
    ))(input)
}

fn sound_name(input: &str) -> IResult<&str, &str> {
    ws(take_while1(|c: char| {
        c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '^' || c == '!' || c == '.'
    }))(input)
}

fn say_command(input: &str) -> IResult<&str, SayCommand> {
    let (input, (name, opts)) = pair(sound_name, many0(say_arg))(input)?;
    let (input, action) = map(action, |c| match c {
        ";" => Action::Synthesize,
        "|" => Action::Concat,
        "" => Action::Synthesize,
        _ => unreachable!(),
    })(input)?;

    let mut saycmd = SayCommand {
        name: name.to_string(),
        action,
        ..Default::default()
    };

    for opt in opts {
        match opt {
            SayArg::Speed(n) => saycmd.speed = n,
            SayArg::Pitch(n) => saycmd.pitch = n,
            SayArg::Wait(n) => saycmd.wait = n,
            SayArg::Start(n) => saycmd.start = n,
            SayArg::Duration(n) => saycmd.duration = Some(n),
            SayArg::Stop => saycmd.stop = true,
        }
    }

    Ok((input, saycmd))
}

fn wait_command(input: &str) -> IResult<&str, f64> {
    terminated(preceded(ws(tag("~w")), ws(double)), ws(action))(input)
}

fn commands(input: &str) -> IResult<&str, Vec<Command>> {
    many1(alt((
        map(say_command, Command::Say),
        map(wait_command, |n| Command::Wait((n * 1000.0) as u32)),
    )))(input)
}

/// A combinator that takes a parser `inner` and produces a parser that also consumes both leading and
/// trailing whitespace, returning the output of `inner`.
fn ws<'a, F: 'a, O, E: ParseError<&'a str>>(
    inner: F,
) -> impl FnMut(&'a str) -> IResult<&'a str, O, E>
where
    F: FnMut(&'a str) -> IResult<&'a str, O, E>,
{
    delimited(multispace0, inner, multispace0)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parse_single_command_without_options() {
        assert_eq!(
            Commands::from_str("a").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .build()
                    .unwrap()
            )])
        );
        assert_eq!(
            Commands::from_str("a;").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .build()
                    .unwrap()
            )])
        );
        assert_eq!(
            Commands::from_str(" a  ; ").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .build()
                    .unwrap()
            )])
        );
        assert_eq!(
            Commands::from_str("a|").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .action(Action::Concat)
                    .build()
                    .unwrap()
            )])
        );
        assert_eq!(
            Commands::from_str(" a  | ").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .action(Action::Concat)
                    .build()
                    .unwrap()
            )])
        );
        assert_eq!(
            Commands::from_str("~w0.5").unwrap(),
            Commands(vec![Command::Wait(500)])
        );
    }

    #[test]
    fn test_parse_single_command_with_options() {
        assert_eq!(
            Commands::from_str("a 50").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .speed(50)
                    .build()
                    .unwrap()
            )])
        );
        assert_eq!(
            Commands::from_str(" a  50 ").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .speed(50)
                    .build()
                    .unwrap()
            )])
        );
        assert_eq!(
            Commands::from_str("a @50").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .speed(50)
                    .build()
                    .unwrap()
            )])
        );
        assert_eq!(
            Commands::from_str("a @50 p10 w0.1 s0.2 d0.3 s").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .speed(50)
                    .pitch(10)
                    .wait(100)
                    .start(200)
                    .duration(Some(300))
                    .stop(true)
                    .build()
                    .unwrap()
            )])
        );
        assert_eq!(
            Commands::from_str("a@50p10w0.1s0.2d0.3s").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .speed(50)
                    .pitch(10)
                    .wait(100)
                    .start(200)
                    .duration(Some(300))
                    .stop(true)
                    .build()
                    .unwrap()
            )])
        );
    }

    #[test]
    fn test_parse_multiple_commands_without_options() {
        assert_eq!(
            Commands::from_str("a; b; c").unwrap(),
            Commands(vec![
                Command::Say(
                    SayCommandBuilder::default()
                        .name("a".to_string())
                        .build()
                        .unwrap()
                ),
                Command::Say(
                    SayCommandBuilder::default()
                        .name("b".to_string())
                        .build()
                        .unwrap()
                ),
                Command::Say(
                    SayCommandBuilder::default()
                        .name("c".to_string())
                        .build()
                        .unwrap()
                ),
            ])
        );
        assert_eq!(
            Commands::from_str("a;b;c;").unwrap(),
            Commands(vec![
                Command::Say(
                    SayCommandBuilder::default()
                        .name("a".to_string())
                        .build()
                        .unwrap()
                ),
                Command::Say(
                    SayCommandBuilder::default()
                        .name("b".to_string())
                        .build()
                        .unwrap()
                ),
                Command::Say(
                    SayCommandBuilder::default()
                        .name("c".to_string())
                        .build()
                        .unwrap()
                ),
            ])
        );
        assert_eq!(
            Commands::from_str("a|b|c|").unwrap(),
            Commands(vec![
                Command::Say(
                    SayCommandBuilder::default()
                        .name("a".to_string())
                        .action(Action::Concat)
                        .build()
                        .unwrap()
                ),
                Command::Say(
                    SayCommandBuilder::default()
                        .name("b".to_string())
                        .action(Action::Concat)
                        .build()
                        .unwrap()
                ),
                Command::Say(
                    SayCommandBuilder::default()
                        .name("c".to_string())
                        .action(Action::Concat)
                        .build()
                        .unwrap()
                ),
            ])
        );
    }

    #[test]
    fn test_parse_multiple_commands_with_options() {
        assert_eq!(
            Commands::from_str("a @50 p10 w0.1 s0.2 d0.3 s;~w1;b @100 p20 w0.2 s0.3 d0.4 s;")
                .unwrap(),
            Commands(vec![
                Command::Say(
                    SayCommandBuilder::default()
                        .name("a".to_string())
                        .speed(50)
                        .pitch(10)
                        .wait(100)
                        .start(200)
                        .duration(Some(300))
                        .stop(true)
                        .build()
                        .unwrap()
                ),
                Command::Wait(1000),
                Command::Say(
                    SayCommandBuilder::default()
                        .name("b".to_string())
                        .speed(100)
                        .pitch(20)
                        .wait(200)
                        .start(300)
                        .duration(Some(400))
                        .stop(true)
                        .build()
                        .unwrap()
                ),
            ])
        );
    }

    #[test]
    fn test_prioritize_latter_option() {
        assert_eq!(
            Commands::from_str("a 10 20").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .speed(20)
                    .build()
                    .unwrap()
            )])
        );
        assert_eq!(
            Commands::from_str("a 10 20 30").unwrap(),
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .speed(30)
                    .build()
                    .unwrap()
            )])
        );
    }

    #[test]
    fn test_parse_fails_for_usual_text() {
        assert!(Commands::from_str("?????????").is_err());
        assert!(Commands::from_str("This is a test").is_err());
    }

    #[test]
    fn test_to_string() {
        assert_eq!(
            Commands(vec![Command::Say(
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .build()
                    .unwrap()
            )])
            .to_string(),
            "a".to_string()
        );
        assert_eq!(
            Commands(vec![
                Command::Say(
                    SayCommandBuilder::default()
                        .name("a".to_string())
                        .speed(50)
                        .pitch(10)
                        .wait(200)
                        .start(300)
                        .duration(Some(400))
                        .stop(true)
                        .build()
                        .unwrap()
                ),
                Command::Wait(1000),
                Command::Say(
                    SayCommandBuilder::default()
                        .name("b".to_string())
                        .speed(100)
                        .pitch(20)
                        .wait(300)
                        .start(400)
                        .duration(Some(500))
                        .stop(true)
                        .build()
                        .unwrap()
                ),
            ])
            .to_string(),
            "a 50 p10 w0.2 s0.3 d0.4 s; ~w1.0; b p20 w0.3 s0.4 d0.5 s".to_string()
        );
        assert_eq!(
            Commands(vec![
                Command::Say(
                    SayCommandBuilder::default()
                        .name("a".to_string())
                        .action(Action::Concat)
                        .build()
                        .unwrap()
                ),
                Command::Say(
                    SayCommandBuilder::default()
                        .name("b".to_string())
                        .action(Action::Concat)
                        .build()
                        .unwrap()
                ),
                Command::Say(
                    SayCommandBuilder::default()
                        .name("c".to_string())
                        .action(Action::Concat)
                        .build()
                        .unwrap()
                ),
            ])
            .to_string(),
            "a| b| c".to_string()
        );
    }
}
