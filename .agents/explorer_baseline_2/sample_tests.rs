fn prod_code() {
    let a = Some(5);
    let b = a.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_something() {
        let a = Some(5);
        let b = a.unwrap();
    }
}

#[test]
fn standalone_test() {
    let a = Some(5);
    let b = a.unwrap();
}
