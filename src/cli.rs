use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "amae")]
#[command(about = "A ultra-fast Rust-based package manager for JS/TS", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new package.json
    Init,
    
    /// Install all dependencies from package.json
    Install {
        /// Fail if the lockfile is out of sync or missing
        #[arg(long)]
        frozen_lockfile: bool,

        /// Skip devDependencies
        #[arg(long)]
        production: bool,
    },

    /// Update dependencies to their latest versions within package.json ranges
    Update {
        /// Specific package to update
        package: Option<String>,
    },

    /// Show outdated dependencies
    Outdated,
    
    /// Add a new dependency
    Add {
        /// Name of the package to add (e.g. lodash)
        package: String,
        
        /// Add as a devDependency
        #[arg(short, long)]
        dev: bool,
    },
    
    /// Remove a dependency
    Remove {
        /// Name of the package to remove
        package: String,
    },
    
    /// Run a script defined in package.json
    Run {
        /// Name of the script to run (e.g. dev)
        script: String,
    },

    /// Run the test script defined in package.json
    Test,

    /// Run the start script defined in package.json
    Start,

    /// Clean local node_modules and lockfile
    Clean,

    /// List installed dependencies
    List,

    /// Prune global CAS store
    Prune,

    /// Show why a package is installed
    Why {
        /// Name of the package to query
        package: String,
    },

    /// Generate shell autocompletion scripts
    Completions {
        /// The shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}
