//! 프로젝트 기획서를 바탕으로 만든 AI 업무 배분안을 검토하고 선택한 업무만 등록한다.
use super::{arr, b, n, num_text, s};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use wasm_bindgen::JsCast;

const BRIEF_BYTES: usize = 48 * 1024;

#[derive(Default)]
struct Panel {
    team: String,
    project: String,
    provider: String,
    brief: Option<Value>,
    plan: Option<Value>,
    selected: HashSet<String>,
    busy: &'static str,
    message: String,
    error: bool,
}

type Project = Rc<RefCell<Panel>>;

thread_local! {
    static ACTIVE: RefCell<Project> = RefCell::new(Project::default());
}

fn current(project: &Project) -> bool {
    ACTIVE.with(|active| Rc::ptr_eq(&active.borrow(), project))
}

pub(super) fn leave_project() {
    ACTIVE.with(|active| *active.borrow_mut() = Project::default());
}

// 협업 탭을 다시 열었을 때 다른 팀원이 바꾼 기획서도 읽는다.
pub(super) fn refresh_on_enter() {
    ACTIVE.with(|active| {
        let project = active.borrow().clone();
        let mut state = project.borrow_mut();
        if state.busy.is_empty() {
            state.brief = None;
            state.error = false;
            state.message.clear();
        }
    });
}

fn args(project: &Project) -> Value {
    let state = project.borrow();
    json!({ "teamId": state.team, "projectId": state.project })
}

fn redraw(project: &Project) {
    if !current(project) { return; }
    let Some(root) = by_id("aiAllocation") else { return };
    let view = super::view();
    if !matches_project(&view, project) { return; }
    let fresh = panel(&view, project);
    if let Some(parent) = root.parent_node() {
        let _ = parent.replace_child(&fresh, &root);
    }
}

fn matches_project(view: &Value, project: &Project) -> bool {
    let state = project.borrow();
    view["teamId"] == state.team && view["project"]["id"] == state.project
}

fn start(project: &Project, progress: &'static str) -> bool {
    if !current(project) || !matches_project(&super::view(), project) { return false; }
    {
        let mut state = project.borrow_mut();
        if !state.busy.is_empty() { return false; }
        state.busy = progress;
        state.message.clear();
        state.error = false;
    }
    redraw(project);
    true
}

fn finish(project: &Project, message: String, error: bool) {
    {
        let mut state = project.borrow_mut();
        state.busy = "";
        state.message = message;
        state.error = error;
    }
    redraw(project);
}

fn load_brief(project: Project) {
    project.borrow_mut().busy = "기획서를 불러오는 중…";
    spawn(async move {
        let response = call("collab:get-brief", args(&project)).await;
        if !current(&project) { return; }
        match response {
            Ok(brief) => {
                let mut state = project.borrow_mut();
                // 재진입 시 같은 파일의 배분안은 계속 검토할 수 있다.
                if state.plan.as_ref().is_some_and(|p| p["briefSnapshot"] != brief) {
                    state.plan = None;
                    state.selected.clear();
                }
                state.brief = Some(brief);
                drop(state);
                finish(&project, String::new(), false);
            }
            Err(error) => finish(&project, error, true),
        }
    });
}

pub(super) fn section(view: &Value) -> web_sys::HtmlElement {
    let project = ACTIVE.with(|active| {
        if !matches_project(view, &active.borrow()) {
            *active.borrow_mut() = Rc::new(RefCell::new(Panel {
                team: s(view, "teamId"),
                project: s(&view["project"], "id"),
                ..Panel::default()
            }));
        }
        active.borrow().clone()
    });
    let should_load = {
        let state = project.borrow();
        state.brief.is_none() && state.busy.is_empty() && !state.error
    };
    if should_load { load_brief(project.clone()); }
    panel(view, &project)
}

fn panel(view: &Value, project: &Project) -> web_sys::HtmlElement {
    let ai = &view["ai"];
    let claude = b(ai, "hasClaudeKey");
    let openai = b(ai, "hasOpenaiKey");
    let provider = {
        let mut state = project.borrow_mut();
        if state.provider == "openai" && !openai || state.provider != "openai" && !claude {
            state.provider = if openai { "openai" } else { "claude" }.into();
        } else if state.provider.is_empty() {
            state.provider = "claude".into();
        }
        state.provider.clone()
    };
    let state = project.borrow();
    let busy = !state.busy.is_empty();
    let brief = state.brief.as_ref().filter(|v| v.is_object());
    let mut box_ = el("section").id("aiAllocation").cls("block allocation")
        .attr("aria-labelledby", "allocationTitle")
        .kid(el("h2").id("allocationTitle").text("AI 업무 배분"));
    let input = el("input").id("allocationBriefInput").attr("type", "file").attr("accept", ".md,.txt").cls("hidden").build();
    {
        let file_input = input.clone();
        let project = project.clone();
        on(&input, "change", move |_| {
            let element: &web_sys::HtmlInputElement = file_input.unchecked_ref();
            let file = element.files().and_then(|list| list.get(0));
            element.set_value("");
            if let Some(file) = file { save_brief(project.clone(), file); }
        });
    }
    let chooser = input.clone();
    let mut file_row = el("div").cls("allocation-file")
        .kid(el("div").cls("allocation-file-info")
            .kid(el("strong").text(brief.and_then(|v| v["name"].as_str()).unwrap_or("프로젝트 기획서")))
            .kid(el("span").cls("hint").text(match brief {
                Some(v) if !b(v, "shared") => "이 기기에 저장됨",
                Some(_) => "팀 프로젝트에 저장됨",
                None => "UTF-8 .md 또는 .txt 한 개 · 최대 48KB",
            })))
        .kid(el("button").attr("type", "button").flag("disabled", busy).text(if brief.is_some() { "파일 바꾸기" } else { "파일 선택" }).click(move |_| chooser.click()))
        .child(input);
    if let Some(brief) = brief {
        file_row = file_row.kid(el("details").cls("allocation-brief")
            .kid(el("summary").text("기획서 내용 보기"))
            .kid(el("pre").text(&s(brief, "text"))));
    }
    box_ = box_.kid(file_row);
    let select = el("select").id("allocationProvider").attr("aria-label", "업무 배분에 사용할 AI").flag("disabled", busy || (!claude && !openai))
        .kid(el("option").attr("value", "claude").flag("disabled", !claude).flag("selected", provider == "claude").text(if claude { "Claude" } else { "Claude · 미연결" }))
        .kid(el("option").attr("value", "openai").flag("disabled", !openai).flag("selected", provider == "openai").text(if openai { "GPT" } else { "GPT · 미연결" })).build();
    let sel = select.clone();
    let p = project.clone();
    on(&select, "change", move |_| {
        if current(&p) {
            p.borrow_mut().provider = val(&sel);
        }
    });
    let p = project.clone();
    box_ = box_.kid(el("p").cls("hint allocation-help").text("계산하면 선택한 AI에 기획서와 프로젝트 업무, 팀원이 공개한 자기소개·역량·가능 시간을 보냅니다."))
        .kid(el("div").cls("allocation-controls").child(select)
            .kid(el("button").id("allocationCalculate").cls("primary").flag("disabled", busy || brief.is_none() || (!claude && !openai))
                 .text("AI 계산").click(move |_| calculate(p.clone()))));
    if !claude && !openai {
        box_ = box_.kid(el("div").cls("allocation-setup")
            .txt("계산하려면 Claude 또는 GPT API 키를 연결해 주세요. ")
            .kid(el("button").cls("link").text("API 연결 설정").click(|_| super::super::show_tab("settings"))));
    }
    let status = if busy { state.busy } else { &state.message };
    box_ = box_.kid(el("div").id("allocationStatus").attr("role", "status").cls(if state.error { "allocation-status err" } else { "allocation-status" }).text(status));
    if state.error && state.brief.is_none() {
        let p = project.clone();
        box_ = box_.kid(el("button").cls("link").flag("disabled", busy).text("기획서 다시 불러오기").click(move |_| {
            load_brief(p.clone());
            redraw(&p);
        }));
    }
    if let Some(plan) = &state.plan {
        box_ = box_.kid(review(view, project, plan, &state.selected, busy));
    }
    box_.build()
}

fn save_brief(project: Project, file: web_sys::File) {
    if !start(&project, "기획서를 저장하는 중…") { return; }
    spawn(async move {
        let result = async {
            let name = file.name();
            if !name.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("txt")) {
                return Err(".md 또는 .txt 파일을 선택해 주세요.".into());
            }
            if file.size() > BRIEF_BYTES as f64 {
                return Err("기획서는 최대 48KB까지 등록할 수 있어요.".into());
            }
            let buffer = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await.map_err(|_| "파일을 읽지 못했어요.".to_string())?;
            let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
            let text = std::str::from_utf8(&bytes).map_err(|_| "기획서를 UTF-8로 저장한 뒤 다시 선택해 주세요.".to_string())?
                .trim_start_matches('\u{feff}');
            if text.trim().is_empty() || text.contains('\0') {
                return Err("내용이 있는 텍스트 기획서를 선택해 주세요.".into());
            }
            if !current(&project) { return Err("프로젝트가 변경되었어요.".into()); }
            let mut payload = args(&project);
            payload["name"] = json!(name);
            payload["text"] = json!(text);
            call("collab:save-brief", payload).await
        }.await;
        if !current(&project) { return; }
        match result {
            Ok(brief) => {
                let mut state = project.borrow_mut();
                state.brief = Some(brief);
                state.plan = None;
                state.selected.clear();
                drop(state);
                finish(&project, "기획서를 저장했어요.".into(), false);
            }
            Err(error) => finish(&project, error, true),
        }
    });
}

fn calculate(project: Project) {
    if !start(&project, "팀원의 역량과 가능 시간을 바탕으로 업무를 계산하는 중…") { return; }
    let mut payload = args(&project);
    payload["provider"] = json!(project.borrow().provider);
    spawn(async move {
        let result = call("collab:ai-plan", payload).await;
        if !current(&project) { return; }
        match result {
            Ok(mut plan) => {
                let mut state = project.borrow_mut();
                if plan["teamId"] != state.team || plan["projectId"] != state.project {
                    drop(state);
                    finish(&project, "프로젝트가 변경되었어요. 다시 계산해 주세요.".into(), true);
                    return;
                }
                state.selected = arr(&plan, "proposals").iter().map(|p| s(p, "id")).collect();
                plan["briefSnapshot"] = state.brief.clone().unwrap_or_default();
                state.plan = Some(plan);
                drop(state);
                finish(&project, String::new(), false);
            }
            Err(error) => finish(&project, error, true),
        }
    });
}

fn review(view: &Value, project: &Project, plan: &Value, selected: &HashSet<String>, busy: bool) -> El {
    let members = arr(view, "members");
    let proposals = arr(plan, "proposals");
    let provider = if plan["provider"] == "openai" { "GPT" } else { "Claude" };
    let mut list = el("div").cls("allocation-review")
        .kid(el("h3").text(&format!("{provider} 배분안 · 업무 {}개", proposals.len())));
    for warning in arr(plan, "warnings") {
        if let Some(text) = warning.as_str() { list = list.kid(el("p").cls("warn").text(text)); }
    }
    for proposal in &proposals {
        let id = s(proposal, "id");
        let project = project.clone();
        let checked_now = selected.contains(&id);
        let input = el("input").attr("type", "checkbox").flag("checked", checked_now).flag("disabled", busy).build();
        let input_ref = input.clone();
        on(&input, "change", move |_| {
            if !current(&project) { return; }
            let mut state = project.borrow_mut();
            if checked(&input_ref) { state.selected.insert(id.clone()); } else { state.selected.remove(&id); }
            if let Some(apply) = by_id("allocationApply") {
                set_disabled(&apply, state.selected.is_empty());
                apply.set_text_content(Some(&apply_label(state.selected.len())));
            }
        });
        let assignee = members.iter().find(|m| m["uid"] == proposal["uid"]).and_then(|m| m["name"].as_str()).unwrap_or("미배정");
        let kind = if proposal["existingTaskId"].is_string() { "기존 업무" } else { "새 업무" };
        let criteria = s(proposal, "doneCriteria");
        let row = el("div").cls("allocation-proposal")
            .kid(el("label").child(input).kid(el("strong").text(&s(proposal, "title"))))
            .kid(el("div").cls("allocation-meta").text(&format!("{assignee} · {}시간 · {kind}", num_text(n(proposal, "estimateHours")))))
            .kid(el("p").text(&s(proposal, "reason")));
        list = list.kid(if criteria.is_empty() { row } else { row.kid(el("div").cls("hint").text(&format!("완료 기준 · {criteria}"))) });
    }
    let p = project.clone();
    list.kid(el("button").id("allocationApply").cls("primary").flag("disabled", busy || selected.is_empty()).text(&apply_label(selected.len())).click(move |_| apply(p.clone())))
}

fn apply_label(count: usize) -> String { format!("선택한 업무 {count}개 반영") }

fn apply(project: Project) {
    let mut payload = args(&project);
    {
        let state = project.borrow();
        let Some(plan) = &state.plan else { return };
        if state.selected.is_empty() { return; }
        payload["planId"] = plan["planId"].clone();
        payload["selected"] = json!(arr(plan, "proposals").iter().map(|r| s(r, "id")).filter(|id| state.selected.contains(id)).collect::<Vec<_>>());
    }
    if !start(&project, "선택한 업무를 반영하는 중…") { return; }
    spawn(async move {
        let result = call("collab:apply-ai-plan", payload).await;
        if !current(&project) { return; }
        match result {
            Ok(view) if matches_project(&view, &project) && matches_project(&super::view(), &project) => {
                {
                    let mut state = project.borrow_mut();
                    state.busy = "";
                    state.message = "선택한 업무를 반영했어요. 아래 작업 목록에서 확인할 수 있어요.".into();
                    state.plan = None;
                    state.selected.clear();
                }
                super::cs(|c| { c.view = Some(view); c.plan = None; c.render_id += 1; });
                super::draw();
            }
            Ok(_) => finish(&project, "선택한 프로젝트 화면을 다시 열어 주세요.".into(), false),
            Err(error) => finish(&project, error, true),
        }
    });
}
