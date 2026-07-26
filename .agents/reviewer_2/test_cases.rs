// Test case 1: Multiple attributes before test function
#[test]
#[should_panic]
fn test_with_multiple_attributes() {
    let x: Option<i32> = None;
    let _ = x.unwrap(); // Edge case 1
}

// Test case 2: Helper function inside #[cfg(test)] module
#[cfg(test)]
mod test_mod {
    fn helper() {
        let x: Option<i32> = None;
        let _ = x.unwrap(); // Edge case 2
    }
}

// Test case 3: Production code with unwrap / panic / expect / todo
fn prod_code() {
    let x: Option<i32> = None;
    let _ = x.unwrap(); // Edge case 3a
    let _ = x.expect("msg"); // Edge case 3b
    panic!("error"); // Edge case 3c
    todo!("implement this"); // Edge case 3d
}

// Test case 4: Other panic macros / methods NOT caught by rule
fn prod_code_uncaught() {
    unimplemented!("uncaught?"); // Edge case 4a
    unreachable!("uncaught?"); // Edge case 4b
    let res: Result<i32, &str> = Err("err");
    let _ = res.unwrap_err(); // Edge case 4c
}

// Test case 5: Unsafe block with SAFETY comment
fn safety_valid() {
    // SAFETY: Valid comment directly above
    unsafe {
        let _ = 1;
    }
}

// Test case 6: Unsafe block with attribute between SAFETY comment and unsafe block
fn safety_invalid_intervening_attr() {
    // SAFETY: Valid rationale
    #[allow(unused_unsafe)]
    unsafe {
        let _ = 1;
    }
}

// Test case 7: Unsafe block with comment INSIDE the block
fn safety_inside_block() {
    unsafe {
        // SAFETY: Rationale inside
        let _ = 1;
    }
}
