//! 고민에서 뽑아낼 수 있는 "일반 주제" 목록.
//! 다른 팀원에게 전달되는 것은 오직 이 주제 id 와, 여기에 미리 써 둔 일반적인 조언 문안뿐이다.
//! 문안에는 특정 사건·날짜·이름·원문 표현이 들어가지 않는다.

pub struct Topic {
    pub id: &'static str,
    pub label: &'static str,
    pub keywords: &'static [&'static str],
    pub advice: &'static [&'static str],
    pub team_proposal: &'static str,
    pub action_request: &'static str,
    pub to_confirm: &'static str,
}

pub static TOPICS: &[Topic] = &[
    Topic {
        id: "time_promise",
        label: "시간 약속",
        keywords: &["시간", "약속", "늦", "지각", "안 왔", "안왔", "안 와", "안와", "펑크", "기다리", "기다렸", "안 나타"],
        advice: &[
            "함께 쓰는 시간에는 서로의 다음 일정도 연결돼 있어. 맞추기 어려워졌다면 미리 알려 줘.",
            "약속한 시간에 맞추기 어렵다면 기다리는 사람이 생기기 전에 알려 주는 것까지가 약속이야.",
            "공유 일정에서 수락한 약속은 지키는 게 기본이야. 어렵다면 미리 연락하고 새 시간을 제안해 봐.",
        ],
        team_proposal: "일정을 잡을 때 참석 확인을 남기고, 변경은 미리 알리는 약속을 만들어 볼까요?",
        action_request: "지연이 예상되면 미리 연락하고 새 시간을 제안하기",
        to_confirm: "상대가 그 시간 약속을 확정된 것으로 알고 있었는지",
    },
    Topic {
        id: "advance_notice",
        label: "사전 연락",
        keywords: &["연락", "답장", "잠수", "읽씹", "말도 없이", "미리", "알려주", "알려 주", "답이 없", "답 없"],
        advice: &[
            "계획이 바뀌었을 때 짧게라도 먼저 알려 주면 기다리는 사람의 하루가 훨씬 편해져.",
            "답을 바로 주기 어려우면 확인했다는 말과 답할 시간을 한 줄로 남겨도 충분해.",
            "사정이 생겼다면 이유와 가능한 새 시간을 함께 전하면 조정하기가 쉬워져.",
        ],
        team_proposal: "연락이 늦어질 때 남길 짧은 신호(확인·예상 시간)를 팀 규칙으로 정해 볼까요?",
        action_request: "계획이 바뀌면 짧게라도 먼저 알리기",
        to_confirm: "상대에게 연락하기 어려운 사정이 있었는지",
    },
    Topic {
        id: "role_clarity",
        label: "역할과 완료 기준",
        keywords: &["맡은", "역할", "담당", "제대로", "대충", "기준", "완료", "덜 했", "덜했", "엉망", "퀄리티"],
        advice: &[
            "맡은 일을 시작하기 전에 \"어디까지 하면 끝인지\" 한 줄로 적어 두면 서로의 기대가 맞춰져.",
            "완료 기준이 애매하다면 혼자 짐작하기보다 먼저 물어보는 편이 빨라.",
            "내가 생각한 \"다 했다\"와 팀이 생각한 \"다 했다\"가 같은지 가끔 확인해 봐.",
        ],
        team_proposal: "각자 맡은 일의 완료 기준을 한 줄씩 적어 맞춰 볼까요?",
        action_request: "맡은 일의 완료 기준을 시작 전에 맞추기",
        to_confirm: "완료 기준을 서로 같은 뜻으로 이해하고 있었는지",
    },
    Topic {
        id: "workload",
        label: "업무량 균형",
        keywords: &["혼자", "다 했", "다했", "떠넘", "떠맡", "몰아", "분담", "무임승차", "안 하고", "안하고", "나만"],
        advice: &[
            "맡은 양이 버겁거나 여유가 생겼다면 일찍 말해 줘. 일을 나누는 건 빠를수록 쉬워.",
            "일의 개수가 같아도 부담은 다를 수 있어. 각자 쓸 수 있는 시간도 함께 이야기해 봐.",
            "도움을 받은 만큼 내가 도울 수 있는 부분도 한 가지 찾아보면 팀이 한결 가벼워져.",
        ],
        team_proposal: "각자 이번 주에 쓸 수 있는 시간과 맡은 일의 무게를 한번 나란히 놓고 볼까요?",
        action_request: "각자 가능한 시간에 맞춰 일을 다시 나누기",
        to_confirm: "상대가 쓸 수 있는 시간과 다른 일정이 어땠는지",
    },
    Topic {
        id: "help_request",
        label: "막힐 때 도움 요청",
        keywords: &["능력", "실력", "못해", "못 해", "모르는 게", "할 줄 모르", "막히", "막혀", "헤매", "느려", "느리"],
        advice: &[
            "처음 맡는 일은 조금씩 진행 상황을 공유하고 막히면 일찍 도움을 요청해 봐.",
            "어디서 막혔는지 한마디만 공유해도 팀 전체 시간을 아낄 수 있어.",
            "모르는 걸 물어보면 일을 더 일찍 끝낼 수 있어.",
        ],
        team_proposal: "막힌 지 30분이 넘으면 공유하는 식의 가벼운 신호를 정해 볼까요?",
        action_request: "작은 단위로 진행을 공유하고 막히면 도움 요청하기",
        to_confirm: "처음 해 보는 일이었는지, 충분한 설명과 준비 시간이 있었는지",
    },
    Topic {
        id: "communication_tone",
        label: "말투와 피드백 방식",
        keywords: &["말투", "무시", "기분 나쁘", "기분나쁘", "짜증내", "비꼬", "화내", "명령", "싸가지", "예의"],
        advice: &[
            "의견을 전할 때 행동과 결과를 짚어서 말하면 같은 내용도 훨씬 잘 전달돼.",
            "급할수록 짧고 부드럽게 말해 봐. 어떤 부분을 확인해야 하는지 한 줄이면 충분해.",
            "다른 의견을 내기 전에 상대의 말을 제대로 이해했는지 한 번 확인해 봐.",
        ],
        team_proposal: "피드백은 사람 대신 결과물에 대해 말하기로 함께 약속해 볼까요?",
        action_request: "사람 대신 결과물에 대해 부드럽게 의견 전하기",
        to_confirm: "상대가 그 말을 어떤 의도로 했는지",
    },
    Topic {
        id: "info_sharing",
        label: "정보와 자료 공유",
        keywords: &["공유", "자료", "파일", "안 알려", "몰랐", "혼자만", "링크", "폴더", "버전"],
        advice: &[
            "새로 알게 된 내용이나 바뀐 파일을 정해 둔 곳에 바로 올려 두면 같은 질문이 줄어들어.",
            "내가 알고 있는 걸 팀도 알고 있는지 가끔 한 번씩 확인해 봐.",
            "최신 버전이 어디 있는지 한 곳에 적어 두면 모두가 덜 헤매.",
        ],
        team_proposal: "자료와 최신 버전을 모아 둘 한 곳을 정해 볼까요?",
        action_request: "자료와 최신 버전을 정해 둔 곳에 바로 올리기",
        to_confirm: "자료를 어디에 공유하기로 했는지 서로 알고 있었는지",
    },
    Topic {
        id: "meeting_participation",
        label: "회의 참여",
        keywords: &["회의", "참여", "의견", "대답 없", "대답없", "조용", "안 듣", "딴짓"],
        advice: &[
            "회의에서 의견이나 질문 하나만 준비해 가도 팀이 내 생각을 훨씬 잘 알 수 있어.",
            "말하기가 어렵다면 회의 전후에 글로 남겨도 좋아.",
            "회의가 끝날 때 \"누가, 무엇을, 언제까지\"를 한 번 소리 내어 맞춰 봐.",
        ],
        team_proposal: "회의 마지막 2분은 할 일·담당·기한을 정리하는 시간으로 정해 볼까요?",
        action_request: "회의 끝에 할 일·담당·기한을 함께 정리하기",
        to_confirm: "상대가 의견을 내기 어려운 이유가 있었는지",
    },
];

/// 자동 반영에서 빼야 할 민감한 내용의 단서 (건강·가족·연애·돈·평가 등)
pub static SENSITIVE_HINTS: &[&str] = &[
    "아파", "아프", "병원", "건강", "우울", "공황", "약 먹", "수술", "가족", "부모", "엄마", "아빠", "집안", "연애", "헤어", "여자친구",
    "남자친구", "애인", "돈", "빚", "알바비", "월급", "성적", "학점", "평가 점수", "인사 평가", "징계",
];

pub fn topic(id: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|t| t.id == id)
}

pub fn detect_topics(text: &str) -> Vec<String> {
    let t = text.to_lowercase();
    let mut hits: Vec<(usize, usize, &str)> = TOPICS
        .iter()
        .enumerate()
        .filter_map(|(i, tp)| {
            let score = tp.keywords.iter().filter(|k| t.contains(*k)).count();
            (score > 0).then_some((score, i, tp.id))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|h| h.2.to_string()).collect()
}

pub fn detect_sensitive(text: &str) -> Vec<String> {
    SENSITIVE_HINTS.iter().filter(|k| text.contains(*k)).map(|s| s.to_string()).collect()
}
