use std::str::FromStr;

use nom::{
    branch::alt,
    bytes::complete::{tag, take_till1, take_while_m_n, take_while1},
    character::{
        complete::{alphanumeric1, char, one_of, satisfy, u32},
        is_alphanumeric,
    },
    combinator::{map, map_res, opt, peek, recognize},
    error::Error,
    multi::{many0, many1_count, separated_list1},
    number::complete::{self, be_u32, double},
    sequence::{pair, preceded, tuple},
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
            wait: 50,
            start: 0,
            duration: None,
            stop: false,
            action: Action::Synthesize,
        }
    }
}

#[derive(Debug)]
enum Command {
    Say(SayCommand),
    Wait(u32),
}

// impl FromStr for Command {
//     type Err = Error<String>;

//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         match parse_command(s).finish() {
//             Ok((_remaining, command)) => Ok(command),
//             Err(Error { input, code }) => Err(Error {
//                 input: input.to_string(),
//                 code,
//             }),
//         }
//     }
// }

#[derive(Debug)]
struct Commands(Vec<Command>);

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

enum SayArg {
    Speed(u32),
    Pitch(u32),
    Wait(u32),
    Start(u32),
    Duration(u32),
    Stop,
    Action(Action),
}

fn speed(i: &str) -> IResult<&str, u32> {
    preceded(opt(char('@')), u32)(i)
}

fn pitch(i: &str) -> IResult<&str, u32> {
    preceded(char('p'), u32)(i)
}

fn wait(i: &str) -> IResult<&str, f64> {
    preceded(char('w'), double)(i)
}

fn start(i: &str) -> IResult<&str, f64> {
    preceded(char('s'), double)(i)
}

fn duration(i: &str) -> IResult<&str, f64> {
    preceded(char('d'), double)(i)
}

fn stop(i: &str) -> IResult<&str, &str> {
    alt((tag("stop"), tag("s")))(i)
}

fn action(i: &str) -> IResult<&str, char> {
    alt((char(';'), char('|')))(i)
}

fn say_arg(input: &str) -> IResult<&str, SayArg> {
    alt((
        map(speed, |n| SayArg::Speed(n)),
        map(pitch, |n| SayArg::Pitch(n)),
        map(wait, |n| SayArg::Wait((n * 1000.0) as u32)),
        map(start, |n| SayArg::Start((n * 1000.0) as u32)),
        map(duration, |n| SayArg::Duration((n * 1000.0) as u32)),
        map(stop, |_| SayArg::Stop),
        map(peek(action), |c| match c {
            ';' => SayArg::Action(Action::Synthesize),
            '|' => SayArg::Action(Action::Concat),
            _ => unreachable!(),
        }),
    ))(input)
}

fn sound_name(input: &str) -> IResult<&str, &str> {
    take_while1(|c: char| c.is_alphanumeric() || c == '-' || c == '_' || c == '^' || c == '!')(input)
}

fn say_command(input: &str) -> IResult<&str, SayCommand> {
    let (input, (name, opts)) = pair(sound_name, many0(say_arg))(input)?;

    let mut saycmd = SayCommand::default();
    saycmd.name = name.to_string();
    for opt in opts {
        match opt {
            SayArg::Speed(n) => saycmd.speed = n,
            SayArg::Pitch(n) => saycmd.pitch = n,
            SayArg::Wait(n) => saycmd.wait = n,
            SayArg::Start(n) => saycmd.start = n,
            SayArg::Duration(n) => saycmd.duration = Some(n),
            SayArg::Stop => saycmd.stop = true,
            SayArg::Action(a) => saycmd.action = a,
        }
    }

    Ok((input, saycmd))
}

fn commands(input: &str) -> IResult<&str, Vec<Command>> {
    separated_list1(
        // alt((tag(";"), tag("|"))),
        tag(";"),
        alt((
            map(say_command, |s| Command::Say(s)),
            map(preceded(tag("~w"), double), |n| {
                Command::Wait((n * 1000.0) as u32)
            }),
        )),
    )(input)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test() {
        // let a = Commands::from_str("a;b;c").unwrap();
        let a = say_command("a s0.5").unwrap();
    }
}
