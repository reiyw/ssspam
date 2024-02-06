use std::{fmt::Write, str::FromStr};

use nom::{
    branch::alt,
    bytes::complete::{tag, take_till, take_while1},
    character::complete::{char, multispace0, u32},
    combinator::{eof, map, opt},
    error::{Error, ParseError},
    multi::{many0, many1},
    number::complete::double,
    sequence::{delimited, pair, preceded},
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
    pub audio_filter: Option<String>,
}

impl Default for SayCommand {
    fn default() -> Self {
        Self {
            name: "".into(),
            speed: 100,
            pitch: 100,
            wait: 0,
            start: 0,
            duration: None,
            stop: false,
            action: Action::Synthesize,
            audio_filter: None,
        }
    }
}

impl ToString for SayCommand {
    fn to_string(&self) -> String {
        let mut s = self.name.clone();
        if self.speed != 100 {
            write!(s, " {}", self.speed).unwrap();
        }
        if self.pitch != 100 {
            write!(s, " p{}", self.pitch).unwrap();
        }
        if self.wait != 0 {
            write!(s, " w{:.1}", (self.wait as f64) / 1000.0).unwrap();
        }
        if self.start != 0 {
            write!(s, " s{:.1}", (self.start as f64) / 1000.0).unwrap();
        }
        if let Some(dur) = self.duration {
            write!(s, " d{:.1}", (dur as f64) / 1000.0).unwrap();
        }
        if self.stop {
            s += " s";
        }
        if let Some(ref af) = self.audio_filter {
            write!(s, " af={af}").unwrap();
        }
        match self.action {
            Action::Synthesize => s += "; ",
            Action::Concat => s += "| ",
        }
        s
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SayCommands(Vec<SayCommand>);

impl SayCommands {
    pub fn iter(&self) -> std::slice::Iter<'_, SayCommand> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn sanitize(&mut self) {
        for cmd in self.0.iter_mut() {
            cmd.pitch = std::cmp::max(cmd.pitch, 1);
            cmd.pitch = std::cmp::min(cmd.pitch, 200);
        }
    }
}

impl IntoIterator for SayCommands {
    type IntoIter = std::vec::IntoIter<Self::Item>;
    type Item = SayCommand;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl FromStr for SayCommands {
    type Err = Error<String>;

    #[tracing::instrument(skip_all)]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match say_commands(s).finish() {
            Ok((_remaining, commands)) => Ok(Self(commands)),
            Err(Error { input, code }) => Err(Error {
                input: input.to_string(),
                code,
            }),
        }
    }
}

impl ToString for SayCommands {
    fn to_string(&self) -> String {
        let mut s: String = self.iter().map(|c| c.to_string()).collect();
        s.truncate(s.len() - 2);
        s
    }
}

impl From<Vec<SayCommand>> for SayCommands {
    fn from(value: Vec<SayCommand>) -> Self {
        Self(value)
    }
}

enum SayArg {
    Speed(u32),
    Pitch(u32),
    Wait(u32),
    Start(u32),
    Duration(u32),
    Stop,
    AudioFilter(String),
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

fn audio_filter(i: &str) -> IResult<&str, &str> {
    ws(preceded(tag("af="), take_till(|c| c == ';' || c == ' ')))(i)
}

fn say_arg(input: &str) -> IResult<&str, SayArg> {
    alt((
        map(speed, SayArg::Speed),
        map(pitch, SayArg::Pitch),
        map(wait, |n| SayArg::Wait((n * 1000.0) as u32)),
        map(start, |n| SayArg::Start((n * 1000.0) as u32)),
        map(duration, |n| SayArg::Duration((n * 1000.0) as u32)),
        map(stop, |_| SayArg::Stop),
        map(audio_filter, |af| SayArg::AudioFilter(af.to_owned())),
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
            SayArg::AudioFilter(af) => saycmd.audio_filter = Some(af),
        }
    }

    Ok((input, saycmd))
}

fn say_commands(input: &str) -> IResult<&str, Vec<SayCommand>> {
    many1(say_command)(input)
}

/// A combinator that takes a parser `inner` and produces a parser that also consumes both leading and
/// trailing whitespace, returning the output of `inner`.
fn ws<'a, F, O, E: ParseError<&'a str>>(inner: F) -> impl FnMut(&'a str) -> IResult<&'a str, O, E>
where
    F: 'a + (FnMut(&'a str) -> IResult<&'a str, O, E>),
{
    delimited(multispace0, inner, multispace0)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parse_single_command_without_options() {
        assert_eq!(
            SayCommands::from_str("a").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .build()
                .unwrap()])
        );
        assert_eq!(
            SayCommands::from_str("a;").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .build()
                .unwrap()])
        );
        assert_eq!(
            SayCommands::from_str(" a  ; ").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .build()
                .unwrap()])
        );
        assert_eq!(
            SayCommands::from_str("a|").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .action(Action::Concat)
                .build()
                .unwrap()])
        );
        assert_eq!(
            SayCommands::from_str(" a  | ").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .action(Action::Concat)
                .build()
                .unwrap()])
        );
    }

    #[test]
    fn test_parse_single_command_with_options() {
        assert_eq!(
            SayCommands::from_str("a 50").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .speed(50)
                .build()
                .unwrap()])
        );
        assert_eq!(
            SayCommands::from_str(" a  50 ").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .speed(50)
                .build()
                .unwrap()])
        );
        assert_eq!(
            SayCommands::from_str("a @50").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .speed(50)
                .build()
                .unwrap()])
        );
        assert_eq!(
            SayCommands::from_str("a @50 p10 w0.1 s0.2 d0.3 s").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .speed(50)
                .pitch(10)
                .wait(100)
                .start(200)
                .duration(Some(300))
                .stop(true)
                .build()
                .unwrap()])
        );
        assert_eq!(
            SayCommands::from_str("a@50p10w0.1s0.2d0.3s").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .speed(50)
                .pitch(10)
                .wait(100)
                .start(200)
                .duration(Some(300))
                .stop(true)
                .build()
                .unwrap()])
        );
    }

    #[test]
    fn test_parse_multiple_commands_without_options() {
        assert_eq!(
            SayCommands::from_str("a; b; c").unwrap(),
            SayCommands(vec![
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("b".to_string())
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("c".to_string())
                    .build()
                    .unwrap(),
            ])
        );
        assert_eq!(
            SayCommands::from_str("a;b;c;").unwrap(),
            SayCommands(vec![
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("b".to_string())
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("c".to_string())
                    .build()
                    .unwrap(),
            ])
        );
        assert_eq!(
            SayCommands::from_str("a|b|c|").unwrap(),
            SayCommands(vec![
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .action(Action::Concat)
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("b".to_string())
                    .action(Action::Concat)
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("c".to_string())
                    .action(Action::Concat)
                    .build()
                    .unwrap(),
            ])
        );
    }

    #[test]
    fn test_parse_multiple_commands_with_options() {
        assert_eq!(
            SayCommands::from_str("a @50 p10 w0.1 s0.2 d0.3 s;b @100 p20 w0.2 s0.3 d0.4 s;")
                .unwrap(),
            SayCommands(vec![
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .speed(50)
                    .pitch(10)
                    .wait(100)
                    .start(200)
                    .duration(Some(300))
                    .stop(true)
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("b".to_string())
                    .speed(100)
                    .pitch(20)
                    .wait(200)
                    .start(300)
                    .duration(Some(400))
                    .stop(true)
                    .build()
                    .unwrap(),
            ])
        );
    }

    #[test]
    fn test_prioritize_latter_option() {
        assert_eq!(
            SayCommands::from_str("a 10 20").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .speed(20)
                .build()
                .unwrap()])
        );
        assert_eq!(
            SayCommands::from_str("a 10 20 30").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .speed(30)
                .build()
                .unwrap()])
        );
    }

    #[test]
    fn test_parse_fails_for_usual_text() {
        assert!(SayCommands::from_str("テスト").is_err());
        assert!(SayCommands::from_str("This is a test").is_err());
    }

    #[test]
    fn test_to_string() {
        assert_eq!(
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_string())
                .build()
                .unwrap()])
            .to_string(),
            "a".to_string()
        );
        assert_eq!(
            SayCommands(vec![
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .speed(50)
                    .pitch(10)
                    .wait(200)
                    .start(300)
                    .duration(Some(400))
                    .stop(true)
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("b".to_string())
                    .speed(100)
                    .pitch(20)
                    .wait(300)
                    .start(400)
                    .duration(Some(500))
                    .stop(true)
                    .build()
                    .unwrap(),
            ])
            .to_string(),
            "a 50 p10 w0.2 s0.3 d0.4 s; b p20 w0.3 s0.4 d0.5 s".to_string()
        );
        assert_eq!(
            SayCommands(vec![
                SayCommandBuilder::default()
                    .name("a".to_string())
                    .action(Action::Concat)
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("b".to_string())
                    .action(Action::Concat)
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("c".to_string())
                    .action(Action::Concat)
                    .build()
                    .unwrap(),
            ])
            .to_string(),
            "a| b| c".to_string()
        );
    }

    #[test]
    fn test_audio_filter() {
        assert_eq!(
            SayCommands::from_str("a af=aecho=0.8:0.88:60:0.4").unwrap(),
            SayCommands(vec![SayCommandBuilder::default()
                .name("a".to_owned())
                .audio_filter(Some("aecho=0.8:0.88:60:0.4".to_owned()))
                .build()
                .unwrap()])
        );

        assert_eq!(
            SayCommands::from_str("a af=aecho=0.8:0.9:1000|1800:0.3|0.25 | b").unwrap(),
            SayCommands(vec![
                SayCommandBuilder::default()
                    .name("a".to_owned())
                    .audio_filter(Some("aecho=0.8:0.9:1000|1800:0.3|0.25".to_owned()))
                    .action(Action::Concat)
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("b".to_owned())
                    .build()
                    .unwrap(),
            ])
        );
    }
}
