{
  description = "Quiz web app: shows a Markdown question to students and collects their answers";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        quiz = pkgs.callPackage ./nix/package.nix { };
        default = quiz;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            openssl # only for generating a test key
          ];
          RUST_BACKTRACE = "1";
        };
      });

      # The module on its own builds the package from the system's nixpkgs;
      # from the flake it gets the one built from the flake's.
      nixosModules.default = { lib, pkgs, ... }: {
        imports = [ ./nix/module.nix ];
        services.quiz-server.package =
          lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.quiz;
      };
    };
}
