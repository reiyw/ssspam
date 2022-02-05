use pest::Parser;

#[derive(Parser)]
#[grammar = "say.pest"]
pub struct SayCommandParser;

#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord, Hash, Builder)]
#[builder(default)]
pub struct SayCommand {
    pub name: String,
    pub speed: u32,
    pub pitch: u32,
    pub wait: u32,
    pub stop: bool,
}

impl SayCommand {
    pub fn new(name: String, speed: u32, pitch: u32, wait: u32, stop: bool) -> Self {
        Self {
            name,
            speed,
            pitch,
            wait,
            stop,
        }
    }
}

impl Default for SayCommand {
    fn default() -> Self {
        Self::new("".into(), 100, 100, 50, false)
    }
}

pub fn parse_say_commands(input: &str) -> Result<Vec<SayCommand>, pest::error::Error<Rule>> {
    let result = SayCommandParser::parse(Rule::cmds, input)?.next().unwrap();
    let mut cmds = Vec::new();
    for cmd in result.into_inner() {
        match cmd.as_rule() {
            Rule::cmd => {
                let mut cmd = cmd.into_inner();
                let name = cmd.next().unwrap().as_str().into();
                let mut saycmd = SayCommandBuilder::default().name(name).build().unwrap();
                for options in cmd {
                    for option in options.into_inner() {
                        match option.as_rule() {
                            Rule::speed => {
                                let start = if option.as_str().starts_with('@') {
                                    1
                                } else {
                                    0
                                };
                                let speed = option.as_str()[start..].parse().unwrap();
                                if (10..=999).contains(&speed) {
                                    saycmd.speed = speed;
                                }
                            }
                            Rule::pitch => {
                                let pitch = option.as_str()[1..].parse().unwrap();
                                if (10..=999).contains(&pitch) {
                                    saycmd.pitch = pitch;
                                }
                            }
                            Rule::wait => {
                                if let Ok(wait) = option.as_str()[1..].parse::<f64>() {
                                    let wait = (wait * 1000.0).round() as u32;
                                    if (10..=30000).contains(&wait) {
                                        saycmd.wait = wait;
                                    }
                                }
                            }
                            Rule::stop => {
                                saycmd.stop = true;
                            }
                            _ => {
                                unreachable!();
                            }
                        }
                    }
                }
                cmds.push(saycmd);
            }
            Rule::EOI => (),
            _ => unreachable!(),
        }
    }
    if cmds.len() > 10 {
        cmds.resize(10, SayCommand::default());
    }
    Ok(cmds)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parser() {
        let cmds = parse_say_commands("a").unwrap();
        assert_eq!(
            cmds,
            vec![SayCommandBuilder::default()
                .name("a".into())
                .build()
                .unwrap()]
        );

        let cmds = parse_say_commands(" a ").unwrap();
        assert_eq!(
            cmds,
            vec![SayCommandBuilder::default()
                .name("a".into())
                .build()
                .unwrap()]
        );

        let cmds = parse_say_commands(" a ;").unwrap();
        assert_eq!(
            cmds,
            vec![SayCommandBuilder::default()
                .name("a".into())
                .build()
                .unwrap()]
        );

        let cmds = parse_say_commands(" a 50 ").unwrap();
        assert_eq!(
            cmds,
            vec![SayCommandBuilder::default()
                .name("a".into())
                .speed(50)
                .build()
                .unwrap()]
        );

        let cmds = parse_say_commands(" a @50 ").unwrap();
        assert_eq!(
            cmds,
            vec![SayCommandBuilder::default()
                .name("a".into())
                .speed(50)
                .build()
                .unwrap()]
        );

        let cmds = parse_say_commands(" a 50; ").unwrap();
        assert_eq!(
            cmds,
            vec![SayCommandBuilder::default()
                .name("a".into())
                .speed(50)
                .build()
                .unwrap()]
        );

        let cmds = parse_say_commands("a; b 50;").unwrap();
        assert_eq!(
            cmds,
            vec![
                SayCommandBuilder::default()
                    .name("a".into())
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("b".into())
                    .speed(50)
                    .build()
                    .unwrap(),
            ]
        );

        let cmds = parse_say_commands("a 1; b 10; c 100; d 999; e 1000").unwrap();
        assert_eq!(
            cmds,
            vec![
                SayCommandBuilder::default()
                    .name("a".into())
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("b".into())
                    .speed(10)
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("c".into())
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("d".into())
                    .speed(999)
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("e".into())
                    .build()
                    .unwrap(),
            ]
        );
    }

    #[test]
    fn test_parser_with_all_options() {
        let cmds = parse_say_commands("a 10 p20 w30").unwrap();
        assert_eq!(
            cmds,
            vec![SayCommandBuilder::default()
                .name("a".into())
                .speed(10)
                .pitch(20)
                .wait(30)
                .build()
                .unwrap()]
        );

        let cmds = parse_say_commands("a w30 p20 10").unwrap();
        assert_eq!(
            cmds,
            vec![SayCommandBuilder::default()
                .name("a".into())
                .speed(10)
                .pitch(20)
                .wait(30)
                .build()
                .unwrap()]
        );

        let cmds = parse_say_commands("a 20 10 p10 p20").unwrap();
        assert_eq!(
            cmds,
            vec![SayCommandBuilder::default()
                .name("a".into())
                .speed(10)
                .pitch(20)
                .build()
                .unwrap()]
        );

        let cmds = parse_say_commands("a p10 20 p20 10").unwrap();
        assert_eq!(
            cmds,
            vec![SayCommandBuilder::default()
                .name("a".into())
                .speed(10)
                .pitch(20)
                .build()
                .unwrap()]
        );

        let cmds = parse_say_commands("a 10 p20 w30; b p10 w20 30").unwrap();
        assert_eq!(
            cmds,
            vec![
                SayCommandBuilder::default()
                    .name("a".into())
                    .speed(10)
                    .pitch(20)
                    .wait(30)
                    .build()
                    .unwrap(),
                SayCommandBuilder::default()
                    .name("b".into())
                    .speed(30)
                    .pitch(10)
                    .wait(20)
                    .build()
                    .unwrap(),
            ]
        );
    }
}
