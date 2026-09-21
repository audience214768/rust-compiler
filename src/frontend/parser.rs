use super::error::*;
use super::ast::AST;
pub struct Parser<'a> {
    src: &'a [u8],
}

pub fn parser_crate(src: &[u8]) -> Result<AST, FrontendError> {
   Ok(AST {})
}