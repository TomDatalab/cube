//! What an organization owner may see of model problems: nothing that
//! reveals another organization's files, paths or cube names.

use std::collections::BTreeMap;

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

/// Shown for a new problem in a cube of another organization.
const FOREIGN_CUBE_LINE: &str =
    "Sua mudança quebra um cubo de outra organização que depende deste; avise o administrador";

/// The cube a compile message is about: its leading context, `<name> cube`
/// or `<name> view`.
fn context_cube(message: &str) -> Option<&str> {
    let first = message.split(" -> ").next()?.split(": ").next()?;
    first
        .strip_suffix(" cube")
        .or_else(|| first.strip_suffix(" view"))
        .filter(|n| !n.is_empty() && !n.contains(char::is_whitespace))
}

/// The names between single quotes in a message.
fn quoted_names(message: &str) -> Vec<&str> {
    message.split('\'').skip(1).step_by(2).collect()
}

/// "... belongs to <slug>" tells which organization holds a name.
fn hide_slug(mut p: Problem) -> Problem {
    if let Some(i) = p.message.rfind(" belongs to ") {
        if !p.message[i + 12..].contains(char::is_whitespace) {
            p.message.truncate(i + 12);
            p.message.push_str("outra organização");
        }
    }
    p
}

/// `problems` as an owner of `own_folder` may see them. `new` are the ones
/// the current change caused; `defs` maps each cube and view name of the
/// candidate model to the source file defining it.
pub fn redact(
    problems: &[Problem],
    new: &[Problem],
    own_folder: &str,
    defs: &BTreeMap<String, String>,
) -> Vec<Problem> {
    let prefix = format!("{own_folder}/");
    let mut out: Vec<Problem> = Vec::new();
    let mut push_once = |out: &mut Vec<Problem>, p: Problem| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for p in problems {
        if !new.contains(p) {
            if p.file.starts_with(&prefix) {
                push_once(&mut out, hide_slug(p.clone()));
            } else {
                push_once(
                    &mut out,
                    Problem {
                        file: String::new(),
                        cube: String::new(),
                        message: GENERIC_LINE.to_string(),
                    },
                );
            }
            continue;
        }
        if let Some(name) = duplicate_name(&p.message) {
            push_once(
                &mut out,
                Problem {
                    file: String::new(),
                    cube: p.cube.clone(),
                    message: duplicate_name_message(name),
                },
            );
            continue;
        }
        let cube = if p.cube.is_empty() {
            context_cube(&p.message).map(str::to_string)
        } else {
            Some(p.cube.clone())
        };
        let home = cube
            .as_ref()
            .and_then(|c| defs.get(c).cloned())
            .or_else(|| (!p.file.is_empty()).then(|| p.file.clone()));
        let own = match (&home, &cube) {
            (Some(h), _) => h.starts_with(&prefix),
            // A cube the definitions do not list (a template, say) and no
            // file: nothing says it is the owner's.
            (None, Some(_)) => false,
            // No file and no cube: fail closed unless every name it quotes
            // is one of the owner's.
            (None, None) => {
                let names = quoted_names(&p.message);
                !names.is_empty()
                    && names
                        .iter()
                        .all(|n| defs.get(*n).is_some_and(|f| f.starts_with(&prefix)))
            }
        };
        if !own {
            push_once(
                &mut out,
                Problem {
                    file: if p.file.is_empty() { String::new() } else { FOREIGN_FILE.to_string() },
                    cube: String::new(),
                    message: FOREIGN_CUBE_LINE.to_string(),
                },
            );
        } else if p.file.is_empty() || p.file.starts_with(&prefix) {
            push_once(&mut out, hide_slug(p.clone()));
        } else {
            push_once(
                &mut out,
                hide_slug(Problem {
                    file: FOREIGN_FILE.to_string(),
                    cube: p.cube.clone(),
                    message: p.message.replace(&p.file, FOREIGN_FILE),
                }),
            );
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
        let newer = p("", "novo problema do cubo 'meu'");
        let out = redact(&[own.clone(), newer.clone()], &[newer.clone()], "on", &defs(&[("meu", "on/x.yml")]));
        assert_eq!(out, vec![own, newer]);
    }

    #[test]
    fn redact_hides_foreign_path() {
        let foreign = p("digital/x.yml", "erro em digital/x.yml na linha 3");
        let out = redact(&[foreign.clone()], &[foreign], "on", &BTreeMap::new());
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
        let out = redact(&[a, b, c, own.clone()], &[], "on", &BTreeMap::new());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].message, GENERIC_LINE);
        assert_eq!(out[1], own);
    }

    #[test]
    fn duplicate_becomes_the_generic_name_message() {
        let d = p("digital/x.yml", "Found duplicate cube name 'fatos'.");
        let out = redact(&[d.clone()], &[d], "on", &BTreeMap::new());
        assert_eq!(out[0].file, "");
        assert_eq!(out[0].message, duplicate_name_message("fatos"));
    }

    fn defs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn foreign_cube_names_never_shown() {
        let n = p("digital/a.yml", "segredo cube -> joins: Cube 'b' not found");
        let d = defs(&[("segredo", "digital/a.yml")]);
        let out = redact(&[n.clone()], &[n], "on", &d);
        assert_eq!(out.len(), 1);
        let all = format!("{} {} {}", out[0].file, out[0].cube, out[0].message);
        assert!(!all.contains("segredo") && !all.contains("digital"), "{all}");
    }

    #[test]
    fn file_less_problem_of_foreign_cube_is_fixed_message() {
        let n = p("", "segredo cube: algo");
        let d = defs(&[("segredo", "digital/a.yml")]);
        let out = redact(&[n.clone()], &[n], "on", &d);
        assert_eq!(out, vec![Problem { file: "".into(), cube: "".into(), message: FOREIGN_CUBE_LINE.into() }]);
    }

    #[test]
    fn own_cube_problem_kept_in_full() {
        let n = p("", "meu cube: algo");
        let d = defs(&[("meu", "on/x.yml")]);
        let out = redact(&[n.clone()], &[n.clone()], "on", &d);
        assert_eq!(out, vec![n]);
    }

    #[test]
    fn context_less_problem_naming_a_foreign_cube_is_fixed() {
        let n = p("", "Circular extends detected for cube 'segredo'");
        let d = defs(&[("segredo", "digital/a.yml")]);
        let out = redact(&[n.clone()], &[n], "on", &d);
        assert_eq!(out, vec![Problem { file: "".into(), cube: "".into(), message: FOREIGN_CUBE_LINE.into() }]);
    }

    #[test]
    fn context_less_problem_naming_only_own_cubes_is_kept() {
        let n = p("", "Circular extends detected for cube 'meu'");
        let d = defs(&[("meu", "on/x.yml")]);
        assert_eq!(redact(&[n.clone()], &[n.clone()], "on", &d), vec![n]);
        let unknown = p("", "Circular extends detected for cube 'nada'");
        assert_eq!(redact(&[unknown.clone()], &[unknown], "on", &d)[0].message, FOREIGN_CUBE_LINE);
    }

    #[test]
    fn attributed_cube_missing_from_defs_is_fixed() {
        // `segredo` lives in a template, so `defs` lacks it; the names it
        // quotes are the owner's.
        let by_context = p("", "segredo cube: Cube 'meu' not found");
        let mut by_field = p("", "Cube 'meu' not found");
        by_field.cube = "segredo".into();
        let d = defs(&[("meu", "on/x.yml")]);
        for n in [by_context, by_field] {
            let out = redact(&[n.clone()], &[n], "on", &d);
            assert_eq!(out, vec![Problem { file: "".into(), cube: "".into(), message: FOREIGN_CUBE_LINE.into() }]);
            let all = format!("{} {} {}", out[0].file, out[0].cube, out[0].message);
            assert!(!all.contains("segredo"), "{all}");
        }
    }

    #[test]
    fn ownership_refusal_hides_the_other_slug() {
        let mut n = p("on/x.yml", "joins fatos belongs to digital");
        n.cube = "meu".into();
        let d = defs(&[("meu", "on/x.yml")]);
        let out = redact(&[n.clone()], &[n], "on", &d);
        assert_eq!(out[0].message, "joins fatos belongs to outra organização");
    }
}
