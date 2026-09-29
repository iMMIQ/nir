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
              git
              rustup
              python3
              bun
              nodejs_24
              chromium
              pkg-config
              # Native desktop player (rodio/cpal links ALSA at build time).
              alsa-lib
              # Runtime bits for local player-linux runs: Vulkan loader with
              # lavapipe for headless software rendering, plus X/Wayland clients.
              vulkan-loader
              mesa
              libxkbcommon
              wayland
              xvfb-run
              # winit dlopens the X11 client libraries at runtime under Xvfb.
              libx11
              libxcursor
              libxi
              libxrandr
              libxrender
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
              # Use the Nix browser in Playwright; allow an explicit override.
              export CHROMIUM="''${CHROMIUM:-${pkgs.chromium}/bin/chromium}"
              # winit dlopens X11/Wayland/Vulkan at runtime; they are not
              # link-time dependencies, so expose them to the dynamic loader.
              export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [
                pkgs.libx11
                pkgs.libxcursor
                pkgs.libxi
                pkgs.libxrandr
                pkgs.libxrender
                pkgs.libxkbcommon
                pkgs.wayland
                pkgs.vulkan-loader
                pkgs.alsa-lib
              ]}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
              # Headless software Vulkan keeps local player-linux runs
              # independent of the host GPU driver.
              export VK_DRIVER_FILES="''${VK_DRIVER_FILES:-${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.${pkgs.stdenv.hostPlatform.qemuArch}.json}"
            '';
          };
        });
    };
}
