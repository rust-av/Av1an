use std::error::Error;

use vergen_git2::{CargoBuilder, Emitter, Git2Builder, RustcBuilder};

fn main() -> Result<(), Box<dyn Error>> {
    let git2 = Git2Builder::default().sha(true).commit_date(true).build()?;
    let cargo = CargoBuilder::default().debug(true).target_triple(true).build()?;
    let rustc = RustcBuilder::default().semver(true).llvm_version(true).build()?;

    Emitter::default()
        .add_instructions(&git2)?
        .add_instructions(&cargo)?
        .add_instructions(&rustc)?
        .emit()?;
    Ok(())
}
