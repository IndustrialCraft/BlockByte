mod ast;
mod eval;
mod variant;

#[cfg(test)]
mod tests {
    use nom_locate::LocatedSpan;
    use nom_recursive::RecursiveInfo;
    use crate::ast;

    #[test]
    fn test() {
        let parsed = ast::expression(LocatedSpan::new_extra("ahoj.add(6)", RecursiveInfo::new())).unwrap().1;
        println!("{:?}", parsed);
    }
}
