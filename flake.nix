{
  description = "NIR development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
    in
    {
      devShells = nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustup
              python3
              bun
              nodejs_24
              pkg-config
            ];

            nativeBuildInputs = [ pkgs.rustPlatform.bindgenHook ];

            # Use one C/C++ toolchain even when the host exports Clang settings.
            shellHook = ''
              # Host libc++ headers must not override GCC's standard library.
              unset CPLUS_INCLUDE_PATH
              export CC="${pkgs.stdenv.cc}/bin/cc"
              export CXX="${pkgs.stdenv.cc}/bin/c++"
              export AR="${pkgs.stdenv.cc.bintools}/bin/ar"
              export CXXSTDLIB=stdc++
            '';
          };
        });
    };
}
