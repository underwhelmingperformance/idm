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

      platform = lib.mkOption {
        type = t.attrsOf t.raw;
        description = "Per-platform build and runtime inputs";
      };

      packagePreCheck = lib.mkOption {
        type = t.lines;
        description = "Package preCheck script";
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
    byKernel = values:
      values.${pkgs.stdenv.hostPlatform.parsed.kernel.name} or values.default;

    rustToolchain = fenixPkgs.stable.withComponents [
      "cargo"
      "clippy"
      "rust-src"
      "rust-analyzer"
      "rustc"
    ];

    rustfmtNightly = fenixPkgs.latest.withComponents ["rustfmt" "rustc"];

    craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;

    platform = let
      default = {
        cargoNativeBuildInputs = [pkgs.pkg-config];
        cargoBuildInputs = [];
        devShellBuildInputs = [pkgs.pkg-config];
        packageRuntimeLibs = [];
      };
    in
      default
      // (byKernel {
        default = {};
        linux = {
          cargoBuildInputs = [
            pkgs.dbus
            pkgs.dbus.lib
          ];
          devShellBuildInputs = default.devShellBuildInputs ++ [pkgs.dbus];
          packageRuntimeLibs = [pkgs.dbus.lib];
        };
        darwin = {
          cargoBuildInputs = [
            pkgs.apple-sdk_15
            pkgs.darwin.libiconv
          ];
        };
      });
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

      inherit platform;

      commonArgs = {
        inherit (config.idm) src;
        strictDeps = true;
        nativeBuildInputs = platform.cargoNativeBuildInputs;
        buildInputs = platform.cargoBuildInputs;
      };

      packagePreCheck = lib.optionalString (platform.packageRuntimeLibs != []) ''
        export LD_LIBRARY_PATH="${lib.makeLibraryPath platform.packageRuntimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
      '';

      cargoArtifacts = craneLib.buildDepsOnly config.idm.commonArgs;
    };
  };
}
