use pest::Parser;

#[derive(Parser)]
#[grammar = "say.pest"]
pub struct SayCommandParser;

#[derive(Debug, PartialEq, Eq, Default, Clone, PartialOrd, Ord)]
pub struct SayCommand {
    pub name: String,
    pub speed: u32,
}

impl SayCommand {
    #[allow(dead_code)]
    fn new(name: String, speed: u32) -> Self {
        Self { name, speed }
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
                let speed = cmd
                    .next()
                    .map(|p| p.as_str().parse().unwrap())
                    .unwrap_or_else(|| 100);
                if (10..=999).contains(&speed) {
                    cmds.push(SayCommand { name, speed });
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
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 100),]);

        let cmds = parse_say_commands(" a ").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 100),]);

        let cmds = parse_say_commands(" a ;").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 100),]);

        let cmds = parse_say_commands(" a 50 ").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 50),]);

        let cmds = parse_say_commands(" a 50; ").unwrap();
        assert_eq!(cmds, vec![SayCommand::new("a".into(), 50),]);

        let cmds = parse_say_commands("a; b 50;").unwrap();
        assert_eq!(
            cmds,
            vec![
                SayCommand::new("a".into(), 100),
                SayCommand::new("b".into(), 50)
            ]
        );

        let cmds = parse_say_commands("a 1; b 10; c 100; d 999; e 1000").unwrap();
        assert_eq!(
            cmds,
            vec![
                SayCommand::new("b".into(), 10),
                SayCommand::new("c".into(), 100),
                SayCommand::new("d".into(), 999),
            ]
        );
    }
    #[test]
    fn test_parser_dedup() {
        let cmds = parse_say_commands("a; a; b; a").unwrap();
        assert_eq!(
            cmds,
            vec![
                SayCommand::new("a".into(), 100),
                SayCommand::new("b".into(), 100),
            ]
        );
    }
}
