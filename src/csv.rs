use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::{is_not, tag, take},
    character::complete::{char, line_ending},
    combinator::{map, value},
    multi::{fold, separated_list0},
    sequence::delimited,
};

#[derive(Clone)]
enum StringFragment<'a> {
    Literal(&'a str),
    EscapedQuote,
}
fn parse_fragment<'a>(input: &'a str) -> IResult<&'a str, StringFragment<'a>> {
    alt((
        value(StringFragment::EscapedQuote, tag("\"\"")),
        map(is_not("\""), StringFragment::Literal),
    ))
    .parse(input)
}

fn parse_string(input: &str) -> IResult<&str, String> {
    let build_string = fold(0.., parse_fragment, String::new, |mut string, fragment| {
        match fragment {
            StringFragment::Literal(s) => string.push_str(s),
            StringFragment::EscapedQuote => string.push('"'),
        };
        string
    });
    alt((
        delimited(char('"'), build_string, char('"')),
        map(is_not(",\"\r\n"), |s: &str| s.to_string()),
        value(String::new(), take(0usize))
    ))
    .parse(input)
}

fn parse_row(input: &str) -> IResult<&str, Vec<String>> {
    separated_list0(tag(","), parse_string).parse(input)
}

pub fn parse_csv(input: &str) -> IResult<&str, Vec<Vec<String>>> {
    separated_list0(line_ending, parse_row).parse(input)
}
