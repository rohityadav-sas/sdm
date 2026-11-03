# Fedora / RHEL RPM. Build on Fedora with:
#   packaging/make-rpm.sh          (see that script: it prepares the source tarball)
# License: SDM itself is yours to choose; the Slint UI toolkit is available under GPL-3.0
# or its royalty-free desktop license. "GPL-3.0-only" is a safe default until you decide.
Name:           sdm
Version:        1.0.0
Release:        1%{?dist}
Summary:        Fast, light download manager with browser integration
License:        GPL-3.0-only
URL:            https://example.invalid/sdm
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  pkgconfig
BuildRequires:  fontconfig-devel
Requires:       xdg-utils
Requires:       xdg-desktop-portal
Recommends:     libnotify

%description
SDM downloads files over up to 32 parallel connections, resumes interrupted
downloads and captures downloads from Chrome, Edge, Brave and Firefox.

%prep
%autosetup

%build
%if 0%{!?sdm_skip_build:1}
cargo build --release --locked
%endif

%install
install -Dm755 target/release/sdm       %{buildroot}%{_bindir}/sdm
install -Dm755 target/release/sdm-host  %{buildroot}%{_bindir}/sdm-host
install -Dm755 target/release/sdm-cli   %{buildroot}%{_bindir}/sdm-cli
install -Dm644 packaging/sdm.desktop    %{buildroot}%{_datadir}/applications/sdm.desktop
for s in 16 32 48 64 128 256 512; do
  install -Dm644 packaging/icons/sdm-$s.png %{buildroot}%{_datadir}/icons/hicolor/${s}x${s}/apps/sdm.png
done

install -Dm644 packaging/icons/sdm.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/sdm.svg

%post
touch --no-create %{_datadir}/icons/hicolor &>/dev/null || :

%postun
if [ $1 -eq 0 ]; then
  touch --no-create %{_datadir}/icons/hicolor &>/dev/null || :
  gtk-update-icon-cache %{_datadir}/icons/hicolor &>/dev/null || :
fi

%files
%{_bindir}/sdm
%{_bindir}/sdm-host
%{_bindir}/sdm-cli
%{_datadir}/applications/sdm.desktop
%{_datadir}/icons/hicolor/*/apps/sdm.png
%{_datadir}/icons/hicolor/scalable/apps/sdm.svg

%changelog
* Fri Oct 23 2026 SDM <sdm@example.invalid> - 1.0.0-1
- First release
