"""Install the Rust toolchain, components and targets declared by the workspace."""
from pathlib import Path
import subprocess
import tomllib


def main():
    root = Path(__file__).resolve().parent.parent
    with (root / "rust-toolchain.toml").open("rb") as source:
        toolchain = tomllib.load(source)["toolchain"]
    command = ["rustup", "toolchain", "install", toolchain["channel"],
               "--profile", toolchain.get("profile", "minimal")]
    for component in toolchain.get("components", []):
        command.extend(["--component", component])
    for target in toolchain.get("targets", []):
        command.extend(["--target", target])
    subprocess.run(command, check=True)


if __name__ == "__main__":
    main()
