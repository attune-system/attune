# Issue 18: indexed pack dependency matrix

Date: 2026-08-20

## Scope

This is a source-derived test matrix for the ten candidates named in issue 18.
It uses the immutable [standard-index snapshot](https://raw.githubusercontent.com/attune-system/index/c9e48439677847797d056efb94ba1c855e188df9/index.json), not repository default branches. No isolated post-fix pack-test logs exist in this checkout. Therefore this report does not say that any pack still fails, or that a proposed change passes.

The Compose full worker starts from `nikolaik/python-nodejs:python3.12-nodejs22-slim` and installs `curl`, CA certificates, and `git` when necessary ([docker-compose.yaml](../../docker-compose.yaml#L637-L665)). It does not install a compiler, Kerberos development files, Ansible, or Packer.

## Test-ready matrix

| Pack and pinned source | Expected condition to exercise | Exact required package, tool, or manifest change | Test result status |
| --- | --- | --- | --- |
| [`activedirectory` at `46138a4`](https://github.com/attune-packs/activedirectory/blob/46138a46583653a427c05b0b728d9cd29b24f215/requirements.txt) | Runtime dependency installation resolves the `pywinrm[kerberos]` extra to a source-built Kerberos extension and the worker lacks `krb5-config` or a C compiler. | Install Debian `build-essential` and `libkrb5-dev` in a Kerberos-capable worker image. `libkrb5-dev` supplies `krb5-config`. | Not run after a fix. |
| [`ansible` at `e21ba6e`](https://github.com/attune-packs/ansible/blob/e21ba6e084b8b93e2a46bb976d6c83ea9cc54cd4/requirements.txt) | An action or test invokes `ansible`, `ansible-playbook`, or another Ansible CLI after the prepared pack runtime installs its declared requirements. The pinned requirements file only comments that Ansible must be installed separately. | Add `ansible-core>=2.21,<2.22` to the pack's `requirements.txt`. For SSH transport only, provide `openssh-client`; add `sshpass` only for supported password SSH. | Not run after a fix. |
| [`git` at `99fca0b`](https://github.com/attune-packs/git/blob/99fca0b4303bf29e73456b2bc08036a1fb14baf3/README.md#requirements) | A selected worker lacks the `git` executable. The Compose full worker already installs it, so this condition applies to other worker images. | Provide Debian `git`, or place the pack on a worker that provides `git`. | Not run after a fix. |
| [`hyperv` at `beb267e`](https://github.com/attune-packs/hyperv/blob/beb267e35bd6790395f2b447a3affd8c2a3468c4/requirements.txt) | Runtime dependency installation resolves the `pywinrm[credssp,kerberos]` extra to a source-built Kerberos extension and the worker lacks `krb5-config` or a C compiler. | Install Debian `build-essential` and `libkrb5-dev` in a Kerberos-capable worker image. | Not run after a fix. |
| [`jira` at `9396b8e`](https://github.com/attune-packs/jira/blob/9396b8e9fe1421abf99beaeb72ee31ebd92e148d/requirements.txt) | Dependency installation cannot obtain a compatible wheel for `jira>=3.8,<4`. | No baseline native package is identified. Capture the resolver error before adding one. | Not run after a fix. |
| [`msexchange` at `57f1e0d`](https://github.com/attune-packs/msexchange/blob/57f1e0da371bb7d74ce1b3aff0c32226021deb24/requirements.txt) | Dependency installation cannot obtain compatible wheels for `exchangelib` or `python-dateutil`. | No baseline native package is identified. Capture the resolver error before adding one. | Not run after a fix. |
| [`packer` at `086168c`](https://github.com/attune-packs/packer/blob/086168cfa90dfcc00bd1819f6e36fabed66c81b5/README.md#requirements) | A selected action worker cannot resolve `packer`, or its configured `PACKER_EXECUTABLE`, on `PATH`. | Install the Packer CLI in a dedicated worker image from HashiCorp's signed APT repository or a verified release binary. | Not run after a fix. |
| [`pagerduty` at `bd4d7c2`](https://github.com/attune-packs/pagerduty/blob/bd4d7c2b2e54dd7edcb8c9fdfb33ea94928ed8fd/requirements.txt) | Dependency installation cannot obtain a compatible `requests` wheel. | No baseline native package is identified. Capture the resolver error before adding one. | Not run after a fix. |
| [`slack` at `2c381f0`](https://github.com/attune-packs/slack/blob/2c381f0d1acc4d79a7d579eb7b1ce96b7d3cd135/requirements.txt) | Dependency installation cannot obtain compatible `requests` or `slack-sdk` wheels. | No baseline native package is identified. Capture the resolver error before adding one. | Not run after a fix. |
| [`sql` at `9b88eb7`](https://github.com/attune-packs/sql/blob/9b88eb7d44e33e7b49d16c9f1f327f9318c1e347/requirements.txt) | `pymssql` lacks a compatible wheel and pip falls back to building it from source. The normal path uses the pack's `psycopg[binary]` and published wheels. | For that source-build fallback only, install Debian `build-essential`, `freetds-dev`, `libssl-dev`, and `libkrb5-dev`. Do not add these to the shared Python worker without a failing log. | Not run after a fix. |

## Recommended validation

Run each pinned pack through the prepared-runtime install and its pack test on the target worker image. Record the resolver or executable error before the change, then repeat after the one matrix change for that row. The result must include the pack commit, worker image digest, Python version, resolved package versions, and command output.

The matrix separates two cases that should not be treated as generic Python dependencies. `ansible-core` belongs in the Ansible pack manifest, while `git` and `packer` are worker executables. Keep compilers, Kerberos headers, FreeTDS headers, and Packer out of the shared Python worker unless this validation proves a shared requirement.

## Sources

- [Pinned standard index](https://raw.githubusercontent.com/attune-system/index/c9e48439677847797d056efb94ba1c855e188df9/index.json)
- [Debian Bookworm `libkrb5-dev` package](https://packages.debian.org/bookworm/libkrb5-dev)
- [Ansible installation guide](https://docs.ansible.com/projects/ansible/latest/installation_guide/intro_installation.html)
- [HashiCorp Packer installation guide](https://developer.hashicorp.com/packer/docs/install)
- [Psycopg binary installation](https://www.psycopg.org/psycopg3/docs/basic/install.html)
- [pymssql installation and source-build requirements](https://pymssql.readthedocs.io/en/stable/intro.html#installation)
