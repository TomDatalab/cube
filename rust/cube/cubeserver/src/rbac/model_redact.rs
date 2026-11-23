//! What an organization owner may see of model problems: nothing that
//! reveals another organization's files, paths or cube names.

use super::model_validate::Problem;

const FOREIGN_FILE: &str = "(arquivo de outra organização)";
const GENERIC_LINE: &str =
    "Há um problema no modelo fora da sua organização; avise o administrador";

/// The message for a cube or view name taken elsewhere on the platform.
pub fn duplicate_name_message(name: &str) -> String {
    format!("O nome de cubo {name} já está em uso na plataforma; escolha outro")
}

/// The name in the loader's "Found duplicate cube name 'x'." message.
fn duplicate_name(message: &str) -> Option<&str> {
    let rest = ["cube", "view"].iter().find_map(|label| {
        message
            .split_once(&format!("Found duplicate {label} name '"))
            .map(|(_, rest)| rest)
    })?;
    rest.split_once('\'').map(|(name, _)| name)
}

/// `problems` as an owner of `own_folder` may see them. `new` are the ones
/// the current change caused.
pub fn redact(problems: &[Problem], new: &[Problem], own_folder: &str) -> Vec<Problem> {
    let prefix = format!("{own_folder}/");
    let mut out = Vec::new();
    let mut generic = false;
    for p in problems {
        let is_new = new.contains(p);
        if is_new {
            if let Some(name) = duplicate_name(&p.message) {
                out.push(Problem {
                    file: String::new(),
                    cube: p.cube.clone(),
                    message: duplicate_name_message(name),
                });
            } else if p.file.is_empty() || p.file.starts_with(&prefix) {
                out.push(p.clone());
            } else {
                out.push(Problem {
                    file: FOREIGN_FILE.to_string(),
                    cube: p.cube.clone(),
                    message: p.message.replace(&p.file, FOREIGN_FILE),
                });
            }
        } else if p.file.starts_with(&prefix) {
            out.push(p.clone());
        } else if !generic {
            generic = true;
            out.push(Problem {
                file: String::new(),
                cube: String::new(),
                message: GENERIC_LINE.to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(file: &str, message: &str) -> Problem {
        Problem {
            file: file.into(),
            cube: String::new(),
            message: message.into(),
        }
    }

    #[test]
    fn redact_keeps_own_and_new() {
        let own = p("on/a.yml", "ruim");
        let newer = p("", "novo sem arquivo");
        let out = redact(&[own.clone(), newer.clone()], &[newer.clone()], "on");
        assert_eq!(out, vec![own, newer]);
    }

    #[test]
    fn redact_hides_foreign_path() {
        let foreign = p("digital/x.yml", "erro em digital/x.yml na linha 3");
        let out = redact(&[foreign.clone()], &[foreign], "on");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].file, "(arquivo de outra organização)");
        assert!(!out[0].message.contains("digital/"), "{}", out[0].message);
    }

    #[test]
    fn old_foreign_become_one_line() {
        let a = p("digital/a.yml", "a");
        let b = p("_platform/b.yml", "b");
        let c = p("", "cubo secreto quebrado");
        let own = p("on/o.yml", "o");
        let out = redact(&[a, b, c, own.clone()], &[], "on");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].message, GENERIC_LINE);
        assert_eq!(out[1], own);
    }

    #[test]
    fn duplicate_becomes_the_generic_name_message() {
        let d = p("digital/x.yml", "Found duplicate cube name 'fatos'.");
        let out = redact(&[d.clone()], &[d], "on");
        assert_eq!(out[0].file, "");
        assert_eq!(out[0].message, duplicate_name_message("fatos"));
    }
}
