Name:           fnf
Version:        0.1.0
Release:        1%{?dist}
Summary:        dnf upgrade wrapper with yay-style colored output

License:        MIT
URL:            https://github.com/sebdotv/fnf
Source0:        %{url}/archive/v%{version}/fnf-%{version}.tar.gz
Source1:        fnf-%{version}-vendor.tar.gz

BuildRequires:  rust-packaging >= 23
BuildRequires:  cargo

%description
fnf (Fancified YUM) is a dnf upgrade wrapper that enhances dnf upgrade
with yay-style colored output: version diffs highlighted by differing
segment, aligned columns, download sizes, repository names, and a Y/n
confirmation prompt before running the actual upgrade.

Binary name: fnf. Run as: fnf upgrade (aliases: up, update).

%prep
%autosetup -n fnf-%{version} -a 1
%cargo_prep -v vendor

%build
%cargo_build

%install
%cargo_install

%check
%cargo_test

%files
%license LICENSE
%doc README.md
%{_bindir}/fnf

%changelog
* Wed Jun 24 2026 sebdotv <sebdotv@gmail.com> - 0.1.0-1
- Initial package
