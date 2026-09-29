# Contributing to firecrab

Thanks for helping improve firecrab.

This guide covers how to set up a development environment, what to change where, and how we review pull requests. For product concepts and operator docs, start with [`public-docs/`](public-docs/README.md).

## A note from the maintainer

<p align="center">
  <img src="assets/icons/contributors.png" alt="Contributors" width="120" />
</p>

**Contributions are welcome.**  
We publish as much information as we can so anyone can join in. Small work counts too — typo fixes, minor bug reports, and similar help are all appreciated. Final review and merge are done by SteelCrab.

**Security and stability come first.**  
This project is complex and aims for features that fit enterprise environments. We care more about security and stability than shipping features for their own sake.

**Please file install failures as Issues.**  
If `install.sh` fails partway through, do not assume it is only your machine. Open an Issue when you can. Environment details, logs, and where it stopped already help a lot.

**Treat each other with respect.**  
Be courteous with other contributors. Prefer positive language and a light emoji over harsh or negative wording. 🙏

**Overlapping work is integrated together.**  
When several people work on similar features, SteelCrab will coordinate the merge so the result is a shared contribution.

**It is okay if maintenance pauses.**  
If life makes it hard to keep a PR going, the maintainer may pick up the work, polish it, and land it. We understand personal circumstances. Showing up and contributing at all is already a big help — a stalled commit or PR does not make the effort meaningless.

## 소개

firecrab은 Firecracker 기반의 단일 호스트 microVM 관리자입니다.
API, 네트워크 helper, CLI, 대시보드로 구성됩니다.
API의 호스트 권한은 최소화하고, 권한이 필요한 네트워크 작업은 helper가 담당합니다.

## 준비

- **공통:** 저장소에 지정된 Rust 툴체인, Node.js 22 이상, npm
- **Linux:** 게스트 실행에는 /dev/kvm과 네트워크 도구가 필요합니다.
- **macOS:** microManager를 사용하며 런타임 검증에는 중첩 가상화가 필요합니다.
- **Windows:** WSL2의 microManager를 사용하며 런타임 검증에는 중첩 가상화가 필요합니다.

단위 테스트와 프런트엔드 빌드만 한다면 전체 설치는 필요하지 않습니다.

## 소스 환경 실행

저장소 루트에서 실행합니다.

**Linux:** 각 명령을 별도 터미널에서 실행합니다.

```sh
# 터미널 1: 네트워크 helper
./scripts/dev-net-helper.sh

# 터미널 2: API
cargo run -p firecrab-api

# 터미널 3: 대시보드
npm run dev --prefix firecrab-frontend
```

**macOS:**

```sh
cargo build -p firecrab-cli --locked
scripts/build-micromanager-macos.sh target/debug/firecrab-micromanager-macos
./target/debug/firecrab service install
```

**Windows PowerShell:**

```powershell
cargo build -p firecrab-cli --locked
.\target\debug\firecrab.exe service install
```

이미 설치된 호스트에서는 service install 대신 service start를 사용합니다.

## 테스트

공통 점검 명령은 다음과 같습니다.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
npm ci --prefix firecrab-frontend
npm run lint --prefix firecrab-frontend
npm run build --prefix firecrab-frontend
python3 scripts/check-doc-links.py
```

브라우저 E2E에서 게스트 부팅을 제외한 검사는 다음과 같습니다.

```sh
npm ci --prefix firecrab-e2e
npm run install-browsers --prefix firecrab-e2e
FIRECRAB_E2E_SKIP_GUEST_BOOT=1 npm test --prefix firecrab-e2e
```

플랫폼별 시나리오, 수동 절차, 기대 결과와 정리 방법은 [한국어 TEST 문서](public-docs/TEST.ko.md)와 [영어 TEST 문서](public-docs/TEST.md)를 참고하세요.

## 커밋

한 가지 변경을 설명하는 짧은 제목을 사용합니다.

```text
fix(api): …
docs: …
ci: …
```

## PR

한 가지 주제에 집중하고 **무엇을 왜 바꿨는지**, 관련 Issue, 테스트 결과를 적어 [Pull Request](https://github.com/SteelCrab/firecrab/pulls)를 엽니다.
적용한 TEST 항목은 PASS/FAILED/WARNING으로 기록해 주세요.

## 이슈

버그나 설치 실패는 재현 절차, 환경, 로그와 함께 [Issue](https://github.com/SteelCrab/firecrab/issues)로 알려주세요.
가능하면 문제 하나당 Issue 하나를 사용합니다.
민감한 보안 문제는 공개하지 말고 유지관리자에게 비공개로 전달해 주세요.

## CI

[CI 워크플로](https://github.com/SteelCrab/firecrab/blob/main/.github/workflows/ci.yml)는 Rust, 프런트엔드, 문서, 설치기 검사와 가능한 자동 시나리오를 실행합니다.

macOS·Windows의 GitHub 호스팅 CI는 중첩 가상화를 제공하지 않아 직접적인 microVM 런타임 검증에는 수동 절차가 필요합니다.
기여 내용에 따라 기여자가 직접 수행한 테스트 결과를 PR 댓글로 남길 수 있습니다.
더 좋은 검증 방법이 있다면 Issue로 제안해 주세요. 함께 개선하겠습니다.

## 라이선스

기여 내용에는 프로젝트와 동일한 [Apache License, Version 2.0](./LICENSE)이 적용됩니다.
