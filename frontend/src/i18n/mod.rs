pub mod zh_cn;

pub use zh_cn as current;

pub fn fill_one(template: &str, value: impl std::fmt::Display) -> String {
    template.replacen("{}", &value.to_string(), 1)
}

pub fn fill_two(
    template: &str,
    first: impl std::fmt::Display,
    second: impl std::fmt::Display,
) -> String {
    let first_pass = template.replacen("{}", &first.to_string(), 1);
    first_pass.replacen("{}", &second.to_string(), 1)
}
