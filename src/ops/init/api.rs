use analysis::{self, DefKind};
use slog::Logger;

use std::collections::{HashSet, VecDeque};
use std::fs::{self, File};
use std::io::prelude::*;

use error;
use cargo::{self, Target};
use Config;
use Result;
use strip_leading_space;

pub fn create(config: &Config, log: &Logger) -> Result<()> {
    // ensure that the api dir exists
    let api_dir = config.api_markdown_path();
    debug!(log, "creating api dir";
    o!("dir" => api_dir.display()));
    fs::create_dir_all(&api_dir)?;

    let metadata = cargo::retrieve_metadata(config.manifest_path())?;
    let target = cargo::target_from_metadata(&log, &metadata)?;

    generate_and_load_analysis(&config, &target, &log)?;

    let host = config.host();
    let crate_name = &target.crate_name();

    let roots = host.def_roots()?;

    // we want to keep track of all modules for the module overview page
    let mut module_set = HashSet::new();

    let id = roots.iter().find(|&&(_, ref name)| name == crate_name);
    let root_id = match id {
        Some(&(id, _)) => id,
        _ => {
            return Err(error::CrateErr {
                crate_name: crate_name.to_string(),
            }.into())
        }
    };

    let root_def = host.get_def(root_id)?;

    let markdown_path = config.api_readme_path();

    debug!(log, "creating README.md for api";
    o!("file" => markdown_path.display()));
    let mut file = File::create(markdown_path)?;

    file.write_all(
        config.handlebars()
            .render(
                "api",
                &json!({"name": crate_name, "docs": strip_leading_space(&root_def.docs)}),
            )?
            .as_bytes(),
    )?;

    let ids = host.for_each_child_def(root_id, |id, _def| id).unwrap();

    // this extra level of indent is for the log to go out of scope
    // this whole thing really needs to be split up into functions, frankly
    {
        let log = log.new(o!("step" => "turning analysis into markdown"));
        info!(log, "starting");
        let mut queue = VecDeque::new();

        for id in ids {
            queue.push_back(id);

            let def = host.get_def(id).unwrap();

            match def.kind {
                DefKind::Mod => (),
                DefKind::Struct => (),
                DefKind::Enum => (),
                DefKind::Trait => (),
                DefKind::Function => (),
                DefKind::Type => (),
                DefKind::Static => (),
                DefKind::Const => (),
                DefKind::Field => (),
                DefKind::Tuple => continue,
                DefKind::Local => continue,
                // The below DefKinds are not supported in rls-analysis
                // DefKind::Union => (String::from("union"), String::from("unions")),
                // DefKind::Macro => (String::from("macro"), String::from("macros")),
                // DefKind::Method => (String::from("method"), String::from("methods")),
                _ => continue,
            };
        }

        while let Some(id) = queue.pop_front() {
            host.for_each_child_def(id, |id, _def| {
                queue.push_back(id);
            })?;

            // Question: we could do this by cloning it in the call to for_each_child_def
            // above/below; is that cheaper, or is this cheaper?
            let def = host.get_def(id).unwrap();

            // if this def is a module, push its id onto the modules list for later
            match def.kind {
                DefKind::Mod => {
                    module_set.insert(id);
                }
                _ => (),
            }

            let template_name = match def.kind {
                DefKind::Mod => "mod",
                DefKind::Struct => "struct",
                DefKind::Enum => "enum",
                DefKind::Trait => "trait",
                DefKind::Function => "function",
                DefKind::Type => "type",
                DefKind::Static => "static",
                DefKind::Const => "const",
                DefKind::Field => continue,
                DefKind::Tuple => continue,
                DefKind::Local => continue,
                // The below DefKinds are not supported in rls-analysis
                // DefKind::Union => (String::from("union"), String::from("unions")),
                // DefKind::Macro => (String::from("macro"), String::from("macros")),
                // DefKind::Method => (String::from("method"), String::from("methods")),
                _ => continue,
            };

            let markdown_path = api_dir.join(&format!("{}.md", def.name));

            let mut file = File::create(markdown_path)?;

            file.write_all(
                config.handlebars()
                    .render(
                        template_name,
                        &json!({"name": def.name, "docs": strip_leading_space(&def.docs)}),
                    )?
                    .as_bytes(),
            )?;
        }

        // now, time for modules:

        #[derive(Debug)]
        struct Module {
            id: analysis::Id,
            children: Vec<Module>,
        }

        let mut krate = Module {
            id: root_id,
            children: Vec::new(),
        };

        // is our call stack smaller than the module depth? hopefully! this is good enough for now
        fn add_children(
            parent: &mut Module,
            possible_children: &HashSet<analysis::Id>,
            host: &analysis::AnalysisHost,
        ) {
            let children: Vec<&analysis::Id> = possible_children
                .iter()
                .filter(|child| {
                    let def = host.get_def(**child).unwrap();
                    def.parent == Some(parent.id)
                })
                .collect();

            // the base case!
            if children.is_empty() {
                return;
            }

            for child in children {
                let mut module = Module {
                    id: *child,
                    children: Vec::new(),
                };

                add_children(&mut module, possible_children, host);

                parent.children.push(module);
            }
        }

        add_children(&mut krate, &module_set, &host);

        // time to write out the markdown

        let markdown_path = config.api_module_overview_path();

        let mut file = File::create(markdown_path)?;

        file.write_all("# Module overview\n\n".as_bytes())?;

        fn print_tree(node: &Module, depth: usize, host: &analysis::AnalysisHost, file: &mut File) {
            let def = host.get_def(node.id).unwrap();

            let name = if def.name.is_empty() {
                "doxidize".to_string()
            } else {
                def.name
            };

            let line = format!(
                "{}* {}\n",
                ::std::iter::repeat("  ")
                    .take(depth)
                    .collect::<Vec<_>>()
                    .join(""),
                name
            );
            file.write_all(line.as_bytes()).unwrap();

            if node.children.is_empty() {
                return;
            }

            for child in &node.children {
                print_tree(child, depth + 1, host, file);
            }
        }

        print_tree(&krate, 0, &host, &mut file);

        info!(log, "done");
    }

    Ok(())
}

/// Generate save analysis data of a crate to be used later by the RLS library later and load it
/// into the analysis host.
fn generate_and_load_analysis(config: &Config, target: &Target, log: &Logger) -> Result<()> {
    let log = log.new(o!("step" => "analysizing your source code"));
    info!(log, "starting");

    cargo::generate_analysis(config, target)?;

    let root_path = config.root_path();
    debug!(log, "analysis complete, loading");
    config.host().reload(root_path, root_path)?;

    info!(log, "done");
    Ok(())
}
