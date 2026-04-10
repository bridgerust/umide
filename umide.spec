Name:           umide-git
Version:        0.4.6.{{{ git_dir_version }}}
Release:        1
Summary:        Unified IDE for Cross-Platform Mobile Development written in Rust
License:        Apache-2.0
URL:            https://github.com/bridgerust/umide

VCS:            {{{ git_dir_vcs }}}
Source:        	{{{ git_dir_pack }}}

BuildRequires:  cargo libxkbcommon-x11-devel libxcb-devel vulkan-loader-devel wayland-devel openssl-devel pkgconf libxkbcommon-x11-devel

%description
UMIDE is a unified IDE for cross-platform mobile development (React Native + Flutter), built in Rust.
It embeds Android Emulator and iOS Simulator directly as panels, eliminating context-switching for mobile developers.

%prep
{{{ git_dir_setup_macro }}}
cargo fetch --locked

%build
cargo build --profile release-lto --bin umide --frozen

%install
install -Dm755 target/release-lto/umide %{buildroot}%{_bindir}/umide
install -Dm644 extra/linux/dev.umide.umide.desktop %{buildroot}/usr/share/applications/dev.umide.umide.desktop
install -Dm644 extra/linux/dev.umide.umide.metainfo.xml %{buildroot}/usr/share/metainfo/dev.umide.umide.metainfo.xml
install -Dm644 extra/images/logo.png %{buildroot}/usr/share/pixmaps/dev.umide.umide.png

%files
%license LICENSE*
%doc *.md
%{_bindir}/umide
/usr/share/applications/dev.umide.umide.desktop
/usr/share/metainfo/dev.umide.umide.metainfo.xml
/usr/share/pixmaps/dev.umide.umide.png

%changelog
* Mon Jan 01 2024 UMIDE contributors
- See full changelog on GitHub
