fn main() {
    // Embedded by `sqlx::migrate!`.
    println!("cargo:rerun-if-changed=migrations");
}
