# Zlack 작업 인수인계

## 목적

Claude provider의 stream start timeout이 반복되어 중단된 이미지/파일 다운로드 개선 작업을 다른 세션이나 모델이 바로 이어서 수행할 수 있도록 현재 상태와 남은 작업을 정리한다.

반복된 오류:

```text
Error: Provider stream start timed out after 90000ms
Error: Provider stream start timed out after 30000ms
Error: Retry failed after 1 attempts: Provider stream start timed out after 30000ms
```

관련 세션 체인:

- `019ffd6f-13f6-7051-bdee-75b754b32a91`
- `019ffd79-9ec5-781a-8627-f188f390e4e2`
- `019ffd96-cbed-7255-965b-370525d08206`
- 마지막 세션은 사용자의 `proceed` 직후 빈 응답으로 종료되어 추가 구현이 전혀 이루어지지 않았다.

## 이미 구현된 기능

### 이미지 우클릭 저장

`src-tauri/preload.js`의 `setupImageSaveMenu()`:

- `<img>`, CSS `background-image`, 이미지 확장자 링크를 감지한다.
- Slack 페이지 컨텍스트에서 `fetch(..., { credentials: 'include' })`하여 인증 쿠키가 필요한 이미지도 읽는다.
- Blob을 base64로 바꾸고 Tauri IPC의 `save_image`를 호출한다.
- 커스텀 컨텍스트 메뉴에 `Save image`를 표시한다.

### Rust 저장 커맨드

`src-tauri/src/downloads.rs`의 `save_image`:

- 실제 OS Downloads 폴더에 저장한다.
- 64 MiB 상한, base64 검증, 안전한 파일명, 이미지 포맷 판별, 덮어쓰기 방지를 제공한다.
- 저장 후 기존 네이티브 알림 백엔드로 `Image saved` 토스트를 표시한다.

### Windows 다운로드 폴더 수정

`src-tauri/src/platform.rs`:

- WebView2 기본 다운로드 위치를 Desktop이 아닌 Downloads로 설정한다.

### 기타 배선

- `src-tauri/src/main.rs`에 `downloads` 모듈과 `downloads::save_image` invoke handler가 등록되어 있다.
- `src-tauri/src/native_notifications.rs`에 fire-and-forget 로컬 알림 helper가 있다.
- `src-tauri/Cargo.toml`에는 `base64 = "0.22"`와 `open = "3"`이 이미 존재한다.
- `CHANGELOG.md`의 Unreleased 섹션에 이미지 저장 기능이 기록되어 있다.

## 남은 사용자 요청

아래 항목들은 조사만 되었고 아직 코드 변경은 없다.

### 1. 일반 파일 다운로드를 fetch -> IPC 방식으로 확장

목표:

- PDF, ZIP 등 비이미지 Slack 첨부 파일도 macOS/Linux를 포함해 Downloads에 저장한다.
- WebView 네이티브 다운로드에 의존하지 않고, 이미지와 같은 인증된 page fetch 경로를 재사용한다.

권장 구현:

1. 먼저 Slack 첨부 파일 다운로드 링크의 실제 DOM/URL 형태를 확인한다.
2. 다운로드 의도가 명확한 링크만 capture 단계에서 가로챈다. 일반 Slack 내부 링크나 외부 링크 처리와 충돌시키지 않는다.
3. 페이지에서 `credentials: 'include'`로 fetch한다.
4. `Content-Disposition`의 `filename*`/`filename`, URL 마지막 경로, 안전한 기본 이름 순으로 파일명을 정한다.
5. Rust에 일반 파일 저장 커맨드를 추가한다. 기존 `save_image`와 파일명 정리·고유 경로·base64 decode·Downloads 저장 로직을 공유하되, 이미지 확장자 강제 로직은 일반 파일에 적용하지 않는다.
6. 저장 완료 토스트는 `File saved`와 실제 파일명을 사용한다.
7. 실패 시 console에만 삼키지 말고 사용자에게 실패를 알릴 수 있는 최소 피드백을 제공한다.

주의:

- 현재 `save_image`는 64 MiB 제한과 base64 JSON IPC를 사용한다. 일반 첨부 파일은 더 클 수 있으므로 허용 크기를 명시적으로 결정해야 한다.
- base64는 메모리 사용량을 약 33% 늘린다. 무제한 파일을 허용하지 않는다.
- 현재 외부 HTTP(S) 링크는 `open_external_url`로 보내는 capture handler가 있으므로, 파일 다운로드 handler의 우선순위와 판별 조건을 테스트해야 한다.
- 고정 sleep이나 타이밍 의존 테스트를 추가하지 않는다.

### 2. 저장 폴더 열기

사용자가 선택항목 1로 승인했다.

권장 구현:

- `open = "3"` 의존성은 이미 있다.
- Rust Tauri command로 Downloads 폴더를 연다.
- 이미지 컨텍스트 메뉴에 `Open Downloads folder` 항목을 추가한다.
- 일반 파일 다운로드 UI에도 동일 기능을 재사용할 수 있게 한다.
- 폴더가 없으면 생성 후 연다.
- 커맨드를 `main.rs`의 `tauri::generate_handler!`에 등록한다.

### 3. 이미지 복사

사용자가 선택항목 3으로 승인했다.

권장 구현:

- 이미지 저장과 동일하게 인증된 fetch로 Blob을 얻는다.
- 컨텍스트 메뉴에 `Copy image`를 추가한다.
- 우선 `navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })])` 경로를 사용하되, 실제 대상 WebView에서 지원 여부를 확인한다.
- unsupported MIME 또는 Clipboard API 미지원 시 명확한 오류를 남긴다.
- 저장과 복사가 같은 URL을 각각 fetch하지 않도록 단일 동작 안에서만 필요한 Blob을 읽고, 불필요한 장기 캐시는 만들지 않는다.
- 텍스트 URL 복사가 아니라 실제 이미지 데이터 복사로 동작해야 한다.

## 권장 작업 순서

1. 동작 변경은 테스트 우선으로 진행한다.
2. `downloads.rs`를 일반 파일 저장까지 확장한다.
3. Downloads 폴더 열기 command를 추가한다.
4. `preload.js`에 일반 파일 다운로드 가로채기, `Open Downloads folder`, `Copy image`를 추가한다.
5. `CHANGELOG.md`의 Unreleased 내용을 갱신한다.
6. 전체 검증 및 실제 앱 QA를 수행한다.

## 관련 파일

- `src-tauri/preload.js`
  - `originalFetch`: 약 97행
  - `tauriInvoke`: 약 395행
  - 외부 링크 capture handler: 약 1370행
  - `setupImageSaveMenu`: 약 1407행
- `src-tauri/src/downloads.rs`
- `src-tauri/src/main.rs`
  - invoke handler: 약 932행
- `src-tauri/src/native_notifications.rs`
- `src-tauri/src/platform.rs`
- `src-tauri/Cargo.toml`
- `CHANGELOG.md`

행 번호는 이후 수정에 따라 달라질 수 있으므로 심볼/함수명으로 다시 찾는다.

## 검증 기준

이전 이미지 저장 구현 당시 확인된 결과:

- `cargo test`: 10/10 통과
- `cargo build`: 성공, 신규 경고 없음
- `node --check preload.js`: 성공

이는 이전 세션 결과이므로 새 변경 후 반드시 다시 실행한다.

```bash
cd src-tauri
cargo test
cargo build
node --check preload.js
```

Behavioral QA:

1. `npm run tauri dev`로 실제 앱을 실행한다.
2. 인증이 필요한 Slack 이미지에서 우클릭한다.
3. `Save image`가 Downloads에 저장되고 토스트가 뜨는지 확인한다.
4. `Copy image` 후 이미지 붙여넣기가 가능한 앱에 실제로 붙여넣는다.
5. `Open Downloads folder`가 OS 파일 탐색기를 여는지 확인한다.
6. PDF/ZIP 등 비이미지 첨부 파일을 다운로드하고 내용이 손상되지 않았는지 확인한다.
7. 일반 Slack 링크와 외부 링크 동작이 회귀하지 않았는지 확인한다.
8. 이미지가 아닌 영역의 기본 우클릭 동작을 방해하지 않는지 확인한다.
9. 잘못된 URL이나 HTTP 실패 시 앱이 멈추지 않고 오류가 관찰되는지 확인한다.

GUI 로그인 세션이 없어 실제 QA를 할 수 없다면, 그 사실과 미검증 항목을 최종 보고에 명확히 남긴다.

## 완료 조건

- 일반 Slack 첨부 파일이 3개 OS에서 Downloads로 저장되는 구현 경로가 존재한다.
- 이미지 컨텍스트 메뉴에서 실제 이미지 복사와 Downloads 폴더 열기가 동작한다.
- Rust 테스트, build, preload 문법 검사가 모두 통과한다.
- 가능한 환경에서는 실제 Slack 앱 surface에서 위 QA를 수행한다.
