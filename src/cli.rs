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
    Install,
    
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
}
