use super::error::*;
use super::ast::Ast;
use super::lexer::lex_all;
use super::token::Token;

use std::vec::Vec;
pub struct Parser<'a> {
    src: &'a [u8],
    tokens: Vec<Token>,
    pos: usize,
    ast: Ast,
}
