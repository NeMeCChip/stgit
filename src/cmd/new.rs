//! `stg new` implementation.

use anyhow::{anyhow, Result};
use clap::{Arg, ArgGroup, ArgMatches, ValueHint};

use super::refresh;

use crate::{
    color::get_color_stdout,
    patchedit,
    patchname::PatchName,
    repo::RepositoryExtended,
    signature::SignatureExtended,
    stack::{Stack, StackStateAccess},
};

pub(super) fn get_command() -> (&'static str, super::StGitCommand) {
    ("new", super::StGitCommand { make, run })
}

fn make() -> clap::Command<'static> {
    let app = clap::Command::new("new")
        .about("Create a new patch at top of the stack")
        .long_about(
            "Create a new, empty patch on the current stack. The new \
             patch is created on top of the currently applied patches, \
             and is made the new top of the stack. Uncommitted changes \
             in the work tree are not included in the patch -- that is \
             handled by stg-refresh.\n\
             \n\
             The given name must be unique in the stack, and may only \
             contain alphanumeric characters, dashes and underscores. \
             If no name is given, one is generated from the first line \
             of the patch's commit message.\n\
             \n\
             An editor will be launched to edit the commit message to \
             be used for the patch, unless the '--message' flag \
             already specified one. The 'patchdescr.tmpl' template \
             file (if available) is used to pre-fill the editor.",
        )
        .arg(
            Arg::new("patchname")
                .help("Name for new patch")
                .value_hint(ValueHint::Other),
        )
        .arg(
            Arg::new("pathspecs")
                .help("Refresh files matching path(s)")
                .long_help(
                    "Refresh files matching path(s). \
                     Specifying paths implies '--refresh'. \
                     Using '--refresh' without any paths will target all modified files.",
                )
                .value_name("path")
                .last(true)
                .multiple_values(true)
                .allow_invalid_utf8(true)
                .forbid_empty_values(true)
                .value_hint(ValueHint::AnyPath)
                .conflicts_with("save-template"),
        )
        .next_help_heading("REFRESH OPTIONS")
        .arg(
            Arg::new("refresh")
                .long("refresh")
                .short('r')
                .help("Refresh new patch with changes from work tree or index")
                .long_help(
                    "Refresh the new patch with changes from work tree. \
                     New patches are empty by default, but with this option \
                     the new patch will capture outstanding changes in the work \
                     tree as if \"stg refresh\" was run. \
                     Use \"--index\" to refresh from the index instead of the work tree.",
                )
                .conflicts_with("save-template"),
        )
        .arg(
            Arg::new("index")
                .long("index")
                .short('i')
                .help("Refresh from index instead of work tree")
                .long_help(
                    "Instead of refreshing the patch with the current \
                     contents of the worktree, use the current contents \
                     of the index.",
                )
                .requires("refresh")
                .conflicts_with_all(&["pathspecs", "submodules", "force"]),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .short('F')
                .help("Force refresh with staged and unstaged changes")
                .long_help(
                    "Force refresh with staged and unstaged changes.\n\
                     \n\
                     By default, if there are staged changes in the index along with \
                     unstaged changes in the work tree, the command will abort. This \
                     option forces the command to proceed using both the staged and \
                     unstaged changes.",
                )
                .requires("refresh")
                .conflicts_with("index"),
        )
        .arg(
            Arg::new("submodules")
                .long("submodules")
                .short('s')
                .help("Include submodules in patch content")
                .requires("refresh"),
        )
        .arg(
            Arg::new("no-submodules")
                .long("no-submodules")
                .help("Exclude submodules in patch content")
                .requires("refresh"),
        )
        .group(ArgGroup::new("submodule-group").args(&["submodules", "no-submodules"]));
    patchedit::add_args(app, true, true)
}

fn run(matches: &ArgMatches) -> Result<()> {
    let repo = git2::Repository::open_from_env()?;
    let branch_name: Option<&str> = None;
    let stack = Stack::from_branch(&repo, branch_name)?;

    repo.check_repository_state()?;
    repo.check_conflicts()?;
    stack.check_head_top_mismatch()?;

    let patchname = if let Some(name) = matches.value_of("patchname") {
        let patchname = name.parse::<PatchName>()?;
        if stack.has_patch(&patchname) {
            return Err(anyhow!("patch `{patchname}` already exists"));
        } else {
            Some(patchname)
        }
    } else {
        None
    };

    let config = repo.config()?;

    let is_refreshing = matches.is_present("refresh") || matches.is_present("pathspecs");

    let tree_id = if is_refreshing {
        refresh::assemble_refresh_tree(&stack, &config, matches, None)?
    } else {
        stack.branch_head.tree_id()
    };

    let parent_id = stack.branch_head.id();

    let (patchname, commit_id) = match patchedit::EditBuilder::default()
        .allow_diff_edit(false)
        .allow_implicit_edit(true)
        .allow_template_save(!is_refreshing)
        .original_patchname(patchname.as_ref())
        .default_author(git2::Signature::make_author(Some(&config), matches)?)
        .override_tree_id(tree_id)
        .override_parent_id(parent_id)
        .edit(&stack, &repo, matches)?
    {
        patchedit::EditOutcome::TemplateSaved(_) => return Ok(()),
        patchedit::EditOutcome::Committed {
            patchname,
            commit_id,
        } => (patchname, commit_id),
    };

    if let Some(template_path) = matches.value_of_os("save-template") {
        let patch_commit = repo.find_commit(commit_id)?;
        std::fs::write(template_path, patch_commit.message_raw_bytes())?;
        return Ok(());
    }

    stack
        .setup_transaction()
        .with_output_stream(get_color_stdout(matches))
        .transact(|trans| trans.new_applied(&patchname, commit_id))
        .execute(&format!("new: {}", &patchname))?;
    Ok(())
}
