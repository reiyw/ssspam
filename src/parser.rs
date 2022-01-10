use pest::Parser;

#[derive(Parser)]
#[grammar = "say.pest"]
pub struct SayCommandParser;

#[derive(Debug, PartialEq, Eq, Default, Clone, PartialOrd, Ord, Hash)]
pub struct SayCommand {
    pub name: String,
    pub speed: u32,
    pub pitch: u32,
}

impl SayCommand {
    pub fn new(name: String, speed: u32, pitch: u32) -> Self {
        Self { name, speed, pitch }
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
                let mut speed = 100;
                let mut pitch = 100;
                for options in cmd {
                    for option in options.into_inner() {
                        match option.as_rule() {
                            Rule::speed => {
                                speed = option.as_str().parse().unwrap();
                            }
                            Rule::pitch => {
                                pitch = option.as_str()[1..].parse().unwrap();
                            }
                            _ => {
                                unreachable!();
                            }
                        }
                    }
                }
                if (10..=999).contains(&speed) && (10..=999).contains(&pitch) {
                    cmds.push(SayCommand { name, speed, pitch });
                }
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
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 100, 100),]);

        let cmds = parse_say_commands(" a ").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 100, 100),]);

        let cmds = parse_say_commands(" a ;").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 100, 100),]);

        let cmds = parse_say_commands(" a 50 ").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 50, 100),]);

        let cmds = parse_say_commands(" a 50; ").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 50, 100),]);

        let cmds = parse_say_commands("a; b 50;").unwrap();
        assert_eq!(
            cmds,
            vec![
                SayCommand::new("a".into(), 100, 100),
                SayCommand::new("b".into(), 50, 100)
            ]
        );

        let cmds = parse_say_commands("a 1; b 10; c 100; d 999; e 1000").unwrap();
        assert_eq!(
            cmds,
            vec![
                SayCommand::new("b".into(), 10, 100),
                SayCommand::new("c".into(), 100, 100),
                SayCommand::new("d".into(), 999, 100),
            ]
        );
    }

    #[test]
    fn test_parser_with_all_options() {
        let cmds = parse_say_commands("a 10 p20").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 10, 20),]);

        let cmds = parse_say_commands("a p20 10").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 10, 20),]);

        let cmds = parse_say_commands("a 20 10 p10 p20").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 10, 20),]);

        let cmds = parse_say_commands("a p10 20 p20 10").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 10, 20),]);

        let cmds = parse_say_commands("a 10 p20; b p10 20").unwrap();
        assert_eq!(
            cmds,
            vec![
                SayCommand::new("a".into(), 10, 20),
                SayCommand::new("b".into(), 20, 10),
            ]
        );
    }
}
