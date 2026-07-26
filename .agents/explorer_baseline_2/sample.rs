fn foo() {
    let x = Some(1);
    let y = x.unwrap();
    let z = x.expect("err");
    panic!("boom");
    todo!("implement me");
}

unsafe fn unsafe_fn() {
    // SAFETY: This is safe because of XYZ
    unsafe {
        let a = 1;
    }

    unsafe {
        let b = 2;
    }
}
