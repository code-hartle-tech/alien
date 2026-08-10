{
  description = "Alien — fan, lighting, turbo and telemetry control for Acer Predator/Nitro laptops";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      # The NixOS module is system-independent, so it lives outside the
      # per-system fold.
      overlay = final: prev: {
        alien = final.callPackage ./packaging/nixos/package.nix { };
      };
    in
    {
      overlays.default = overlay;

      nixosModules.default = { pkgs, ... }: {
        imports = [ ./packaging/nixos/module.nix ];
        services.alien.package = nixpkgs.lib.mkDefault
          (pkgs.callPackage ./packaging/nixos/package.nix { });
      };
    }
    // flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        alien = pkgs.callPackage ./packaging/nixos/package.nix { };
      in
      {
        packages = {
          inherit alien;
          default = alien;
        };

        apps = {
          default = flake-utils.lib.mkApp { drv = alien; name = "alien"; };
          tui = flake-utils.lib.mkApp { drv = alien; name = "alien-tui"; };
          gui = flake-utils.lib.mkApp { drv = alien; name = "alien-gui"; };
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            pkg-config
            # The GUI needs these at build and run time; the CLI, TUI and
            # daemon deliberately need none of them, which is what keeps a
            # headless install small.
            libGL
            libxkbcommon
            wayland
          ];
          # winit dlopens libwayland-client at runtime rather than linking it,
          # so it has to be on the loader path or the GUI dies with
          # NoWaylandLib despite the library being present in the shell.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [
            libGL
            libxkbcommon
            wayland
          ]);
        };
      });
}
