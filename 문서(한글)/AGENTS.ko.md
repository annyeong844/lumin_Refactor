# Lumin v2 저장소 규칙

이 한국어 문서가 저장소 규칙의 정본이다. [AGENTS.md](../AGENTS.md)는 영어 번역본이며, 두 문서가 다르면 이 문서를 따른다.

## 프로젝트 정체성

Lumin v2는 Rust 네이티브 저장소 분석 엔진이자, 하나의 gate ID로 pre-write와 post-write를 연결하는 영속 쓰기 게이트다.

## 읽는 순서

[WORKBOARD.md](../WORKBOARD.md)에서 시작해 현재 작업에 연결된 소유 문서만 읽는다.

## 규칙

1. **SSOT와 불확실성:** 제품 계약 -> 시스템 청사진 -> 담당 아키텍처 문서 -> 활성 슬라이스 -> 이 문서 순서를 따른다. 계약이나 소유자가 불명확하면 수정 전에 멈추고 Workboard에서 소유 문서를 찾는다. 외부 레거시 Lumin 저장소와 코퍼스는 보조 증거로만 사용하고 계약 소유자로 삼지 않는다.
2. **Fail-closed:** 누락, 오래됨, 미지원, 불투명, 실패, 잘림을 clean이나 0건으로 바꾸지 않는다. 조용한 폴백은 금지한다.
3. **구현 전 동결:** 아키텍처 변경은 설계 검토 1회와 독립 적대적 검토 1회를 거쳐 동결한다. 그전에는 구현하지 않으며, MVP 아키텍처, 수평 스캐폴딩, 빈 미래 크레이트, 임시 소유자, 두 번째 엔진을 만들지 않는다.
4. **물리적 소유권:** Cargo 의존성, 가시성, 프로젝트 소유 타입, 아키텍처 검사로 경계를 강제한다. 서드파티 타입은 소유 크레이트 안에 두고, 도메인 사실은 단계 간 JSON이 아니라 프로세스 내부 타입 모델로 전달한다.
5. **결정론적 실행:** [ARCH-001](../architecture/001-execution-and-ownership.md)을 따른다. 워커는 불변 입력을 소비하고 소유한 출력을 반환하며, 소유자가 정한 하나의 결정론적 병합 단계만 결과를 합친다. 두 번째 스케줄러나 풀을 추가하지 않으며, `jobs=1`과 `jobs=N`은 같은 의미적 증거를 만들어야 한다.
6. **영속 pre/post:** pre-write는 gate ID를 반환하고 post-write는 그 ID를 반드시 요구한다. intent JSON이나 전송용 파일을 만들지 않는다. 쓰기/쓰기 및 쓰기/의미 읽기 충돌은 차단한다. 결과 전송 전에는 저장 트랜잭션 잠금·카탈로그 게시 가드·작업 생존 잠금을 해제하되, `Active` gate의 영속 논리 path lease는 close 또는 abandon까지 유지한다.
7. **의존성 격리:** 비용이 큰 의존성은 [ARCH-000](../architecture/000-system-blueprint.md)이 지정한 소유 크레이트 안에 둔다. 새 의존성에는 측정된 제품 가치와 빌드, 크기, unsafe, 전이 비용 검토가 필요하다. 런타임 Cargo, Node 분석 의존성, 소스 폴백은 [PRODUCT-000](../specs/000-product-contract.md)과 ARCH-000에 따라 금지한다.
8. **동작 검증:** 기대 결과는 명세와 구현에서 독립적으로 작성한 코퍼스 정답에서 가져오며, 현재 구현이나 레거시 출력에서 복사하지 않는다. 핵심 경로, 현실적인 엣지, 필수 hard-stop을 검증한다. CI 통과를 위해 단언을 약화하거나 임의 cap/timeout을 추가하거나 실패를 삼키거나 검사를 skip하지 않는다. 파일·함수 존재만 확인해 RED를 만드는 스캐폴딩 테스트도 금지한다.
9. **검증 범위:** push 전에는 영향 범위의 로컬 검사만 실행하고, 공유 코어 변경이나 CI 진단일 때만 전체 로컬 매트릭스를 실행한다. 공개 CI가 clean locked build, 전체 코퍼스와 결정론, 패키지, 의존성 정책의 병합 권한을 가진다. CI 실패 시 실패한 주변만 로컬에서 재현한다.
10. **현재 변경만 닫기:** 소유 사실이 바뀐 경우에만 소유 명세와 Workboard를 갱신한다. 생성 출력을 제거하고 정확한 검사와 한계를 보고한다. 무관한 정리, 레거시 모듈 통복사, 활성 슬라이스 밖의 사용자 작업 변경을 섞지 않는다.

## 검증 매트릭스

- **문서만 변경:** 로컬에서 링크, 포맷/공백, 아키텍처 정합성을 검사하고, 공개 CI는 clean checkout에서 문서 검사를 반복한다.
- **크레이트 하나 또는 코퍼스 사례:** 로컬에서 `cargo fmt`, 범위가 좁은 Clippy/테스트, 영향받는 코퍼스와 경계를 검사하고, 공개 CI는 전체 workspace, 코퍼스, 결정론, 의존성 정책을 검사한다.
- **공유 코어 또는 패키징:** 로컬에서 영향받는 workspace 전체와 패키지 smoke를 실행하되 전체 로컬 매트릭스는 진단 시에만 실행한다. 공개 CI는 locked Windows/Linux 빌드와 동작 패키지 probe를 실행한다.

## Lumin 작업 종료 워크플로우

1. Rust 소스 작업 전에는 외부 legacy Lumin lab의 Rust `pre-write`를 실행하고 invocation-specific advisory를 보존한다. 생성 intent는 stdin으로만 전달하며 저장소 안에 쓰지 않는다.
2. 구현과 범위 테스트가 끝나면 broad Cargo 검사보다 먼저 같은 advisory로 `post-write`를 실행한다. 누락된 advisory나 scan-range 불일치는 clean으로 해석하지 않는다.
3. 현재 TODO 전체가 끝난 뒤 locked Cargo 테스트·Clippy·fmt를 통과시키고, 외부 lab에서 `full --rust-analyzer`를 실행한다. audit 출력은 이 저장소 밖에 둔다.
4. `manifest.rustAnalysis.status`, scan scope, parse/skipped file, `rust-analyzer-health.latest.json.summary.syntaxReviewOpaqueSurfaces`를 먼저 확인한다. 그다음 `checklist-facts.json`, `fix-plan.json`, Rust clone/shape/unused-definition 증거와 Rust 내장 체크리스트를 함께 읽는다. JS/TS artifact를 Rust 부재 증거로 쓰지 않는다.
5. Grounded 오류·중복·dead 정의·경계 위반은 실제 소스를 정독한 뒤 수정한다. `unknown`/`degraded`를 clean으로 바꾸거나 `ReviewOnly` finding을 숨기거나, 의미적 동치 확인 없이 clone을 합치지 않는다.
6. Full audit 뒤 수정이 생기면 새 pre/post 쌍으로 별도 변경 트랜잭션을 열고 영향 테스트와 full Rust audit을 다시 실행한다. Grounded 병합 차단/수정 요구가 남지 않을 때만 작업을 종료한다.

이 워크플로우에서 legacy Lumin은 외부 관측·리뷰 도구일 뿐이며 제품 계약이나 새 구현의 코드 소유자가 아니다.
