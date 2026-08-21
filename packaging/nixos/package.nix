{ lib
, rustPlatform
, pkg-config
, libGL
, libxkbcommon
, wayland
, libx11
, libxcursor
, libxi
, libxrandr
, makeWrapper
}:

rustPlatform.buildRustPackage {
  pname = "alien";
  version = "0.6.0";

  src = lib.cleanSourceWith {
    src = ../..;
    # The derivation depends only on the Rust workspace, application assets,
    # and installed package files. Keeping launch media and documentation out
    # means a screenshot or social export cannot rebuild the privileged daemon
    # or change the installed runtime closure.
    filter = path: type:
      let
        rel = lib.removePrefix (toString ../.. + "/") (toString path);
        base = baseNameOf (toString path);
        container = type == "directory" && builtins.elem rel [
          "code"
          "code/tools"
          "assets"
          "packaging"
          "packaging/shared"
        ];
        buildInput = builtins.elem rel [
          "LICENSE"
          "NOTICE"
          "code/Cargo.toml"
          "code/Cargo.lock"
          "code/tools/alien-launch"
        ]
          || lib.hasPrefix "code/alien-" rel
          || lib.hasPrefix "assets/" rel
          || lib.hasPrefix "packaging/shared/" rel;
        metadata = base == ".DS_Store" || lib.hasPrefix "._" base;
      in !metadata && (rel == "" || container || buildInput);
  };

  sourceRoot = "source/code";
  cargoLock.lockFile = ../../code/Cargo.lock;

  nativeBuildInputs = [ pkg-config makeWrapper ];
  buildInputs = [ libGL libxkbcommon wayland libx11 libxcursor libxi libxrandr ];

  # The unit tests are pure logic and run anywhere. Anything that touches
  # firmware needs root and real hardware, so it is not part of the build.
  doCheck = true;

  postInstall = ''
    # The launcher: GUI -> TUI -> CLI help. Bound to the vendor's
    # PredatorSense key, and the right target for a .desktop action too.
    install -Dm755 tools/alien-launch $out/bin/alien-launch

    install -Dm644 ../packaging/shared/tech.hartle.Alien.desktop \
      $out/share/applications/tech.hartle.Alien.desktop
    install -Dm644 ../assets/tech.hartle.Alien.svg \
      $out/share/icons/hicolor/scalable/apps/tech.hartle.Alien.svg
    install -Dm644 ../packaging/shared/60-alien.rules \
      $out/lib/udev/rules.d/60-alien.rules
    install -Dm644 alien-gui/assets/fonts/Archivo-OFL.txt \
      $out/share/licenses/alien/Archivo-OFL.txt
    install -Dm644 alien-gui/assets/fonts/IBMPlex-OFL.txt \
      $out/share/licenses/alien/IBMPlex-OFL.txt
    install -Dm644 ../LICENSE \
      $out/share/licenses/alien/GPL-2.0-or-later.txt
    install -Dm644 alien-gui/assets/fonts/README.md \
      $out/share/licenses/alien/README.md
    install -Dm644 ../NOTICE $out/share/doc/alien/NOTICE

    # winit dlopens both its Wayland and X11 backends rather than linking them,
    # so rpath is not enough. Alien selects X11 on proprietary-NVIDIA systems
    # to avoid a native-Wayland EGL swap-buffer stall. Only the GUI needs these
    # libraries; wrapping the CLI and daemon would enlarge headless closures.
    wrapProgram $out/bin/alien-gui \
      --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath [
        libGL libxkbcommon wayland libx11 libxcursor libxi libxrandr
      ]}
  '';

  meta = with lib; {
    description = "Fan, lighting, performance and telemetry control for Acer Predator systems";
    homepage = "https://alien.hartle.tech";
    license = licenses.gpl2Plus;
    platforms = platforms.linux;
    mainProgram = "alien";
  };
}
