use std::io::Write;
use std::str::FromStr;

use clap::{App, Arg, ArgMatches};
use indexmap::IndexSet;

use crate::{
    error::Error,
    patchname::PatchName,
    stack::{ConflictMode, Stack, StackTransaction},
};

use super::StGitCommand;

pub(super) fn get_command() -> (&'static str, StGitCommand) {
    ("goto", StGitCommand { get_app, run })
}

fn get_app() -> App<'static> {
    App::new("goto")
        .about("Go to patch by pushing or popping as necessary")
        .arg(&*crate::argset::KEEP_ARG)
        .arg(&*crate::argset::MERGED_ARG)
        .arg(&*crate::color::COLOR_ARG)
        .arg(
            Arg::new("patch")
                .help("Patch to go to")
                .required(true)
                .validator(PatchName::from_str)
                .forbid_empty_values(true),
        )
}

fn run(matches: &ArgMatches) -> super::Result {
    let patchname: PatchName = matches.value_of_t("patch").unwrap();
    let repo = git2::Repository::open_from_env()?;
    let stack = Stack::from_branch(&repo, None)?;

    let opt_keep = matches.is_present("keep");
    let opt_merged = matches.is_present("merged");

    let conflicts_okay = false;
    stack.check_repository_state(conflicts_okay)?;
    stack.check_head_top_mismatch()?;
    if !opt_keep {
        stack.check_index_clean()?;
        stack.check_worktree_clean()?;
    }

    let patchname = if stack.state.patches.contains_key(&patchname) {
        if stack.state.hidden.contains(&patchname) {
            Err(Error::Generic("Cannot goto a hidden patch".to_string()))
        } else {
            Ok(patchname)
        }
    } else {
        let similar_names: Vec<&PatchName> = stack
            .all_patches()
            .filter(|pn| strsim::jaro_winkler(pn.as_ref(), patchname.as_ref()) > 0.75)
            .collect();

        if !similar_names.is_empty() {
            println!("Possible patches:");
            for pn in similar_names {
                println!("  {}", pn);
            }
            Err(Error::Generic(format!(
                "ambiguous patch name `{}`",
                &patchname
            )))
        } else if patchname.len() >= 4 && git2::Oid::from_str(patchname.as_ref()).is_ok() {
            let oid_prefix: &str = patchname.as_ref();
            let oid_prefix: String = oid_prefix.to_ascii_lowercase();
            let oid_matches: Vec<&PatchName> = stack
                .state
                .patches
                .iter()
                .filter_map(|(pn, desc)| {
                    if desc.commit.id().to_string().starts_with(&oid_prefix) {
                        Some(pn)
                    } else {
                        None
                    }
                })
                .collect();

            match oid_matches.len() {
                0 => Err(Error::Generic(format!(
                    "no patch associated with `{}`",
                    &patchname
                ))),
                1 => Ok(oid_matches[0].clone()),
                _ => {
                    println!("Possible patches:");
                    for pn in oid_matches {
                        println!("  {}", pn);
                    }
                    Err(Error::Generic(format!(
                        "ambiguous commit id `{}`",
                        &patchname
                    )))
                }
            }
        } else {
            Err(Error::Generic(format!(
                "patch `{}` does not exist",
                &patchname
            )))
        }
    }?;

    let discard_changes = false;
    let use_index_and_worktree = true;

    let trans_context = StackTransaction::make_context(
        stack,
        ConflictMode::Disallow,
        discard_changes,
        use_index_and_worktree,
    );

    let exec_context = trans_context.transact(|trans| {
        if let Some(pos) = trans.applied().iter().position(|pn| pn == &patchname) {
            let to_pop: IndexSet<PatchName> = trans.applied()[pos + 1..].iter().cloned().collect();
            trans.pop_patches(|pn| to_pop.contains(pn));
            Ok(())
        } else {
            let pos = trans
                .unapplied()
                .iter()
                .position(|pn| pn == &patchname)
                .expect("already determined patch exists and not hidden or applied");

            let to_apply: Vec<PatchName> = trans.unapplied()[0..pos + 1].to_vec();

            let merged = if opt_merged {
                trans.check_merged(&to_apply)?
            } else {
                vec![]
            };

            let mut stdout = crate::color::get_color_stdout(matches);
            let mut _color_spec = termcolor::ColorSpec::new();

            if opt_merged {
                if merged.len() == 1 {
                    writeln!(stdout, "Found 1 patch merged upstream")?;
                } else {
                    writeln!(stdout, "Found {} patches merged upstream", merged.len())?;
                }
            }

            for patchname in &to_apply {
                let already_merged = merged.contains(&patchname);
                trans.push_patch(patchname, already_merged)?;
            }

            Ok(())
        }
    });

    exec_context.execute("goto")?;

    Ok(())
}
