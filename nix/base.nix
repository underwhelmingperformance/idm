{
  inputs,
  lib,
  flake-parts-lib,
  ...
}: {
  options.perSystem = flake-parts-lib.mkPerSystemOption (_: let
    t = lib.types;
  in {
    options.idm = {
      pkgs = lib.mkOption {
        type = t.raw;
        description = "Nixpkgs instance";
      };

      rustToolchain = lib.mkOption {
        type = t.package;
        description = "Rust toolchain for building";
      };

      rustfmtNightly = lib.mkOption {
        type = t.package;
        description = "Nightly rustfmt for unstable options";
      };

      rustfmtBin = lib.mkOption {
        type = t.str;
        description = "Path to nightly rustfmt binary";
      };

      craneLib = lib.mkOption {
        type = t.raw;
        description = "Crane library configured with toolchain";
      };

      src = lib.mkOption {
        type = t.path;
        description = "Filtered source for building";
      };

      commonArgs = lib.mkOption {
        type = t.attrsOf t.raw;
        description = "Common arguments for crane builds";
      };

      cargoArtifacts = lib.mkOption {
        type = t.package;
        description = "Pre-built cargo dependencies";
      };
    };
  });

  config.perSystem = {
    config,
    system,
    ...
  }: let
    pkgs = inputs.nixpkgs.legacyPackages.${system};
    fenixPkgs = inputs.fenix.packages.${system};

    rustToolchain = fenixPkgs.stable.withComponents [
      "cargo"
      "clippy"
      "rust-src"
      "rust-analyzer"
      "rustc"
    ];

    rustfmtNightly = fenixPkgs.latest.withComponents ["rustfmt" "rustc"];

    craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
  in {
    idm = {
      inherit pkgs rustToolchain rustfmtNightly craneLib;

      rustfmtBin = lib.getExe' rustfmtNightly "rustfmt";

      src = let
        rawSrc = craneLib.path {path = inputs.self;};
        snapFilter = path: _type: builtins.match ".*\\.snap$" path != null;
      in
        lib.cleanSourceWith {
          src = rawSrc;
          filter = path: type:
            (snapFilter path type) || (craneLib.filterCargoSources path type);
        };

      commonArgs = {
        inherit (config.idm) src;
        strictDeps = true;
        buildInputs = lib.optionals pkgs.stdenv.isDarwin [
          pkgs.apple-sdk_15
          pkgs.darwin.libiconv
        ];
      };

      cargoArtifacts = craneLib.buildDepsOnly config.idm.commonArgs;
    };
  };
}
