// Google 할 일: 내가 수락한 사이 작업을 "사이 · 프로젝트" 목록에 올리고, 완료 상태를 양쪽으로 맞춘다.
//
// 동기화 기준
//   - 대상: 나에게 배정되어 내가 수락한 작업만 (다른 사람 작업, 미수락 작업은 올리지 않음)
//   - 완료: 어느 쪽에서든 완료하면 다른 쪽도 완료 (사이 쪽은 팀 전체에 보이는 상태)
//   - 제목: 사이 쪽이 기준. Google 에서 제목을 바꿔도 다음 동기화 때 사이 제목으로 돌아간다
//   - Google 에서 지운 할 일은 다시 만들지 않는다 (사용자가 지운 뜻을 존중)
//   - 중복 방지: 사이 작업 id ↔ Google 할 일 id 연결표를 이 기기에 보관

const TASKS = 'https://tasks.googleapis.com/tasks/v1';

async function ensureList(api, title) {
  const lists = await api('GET', `${TASKS}/users/@me/lists?maxResults=100`);
  const found = (lists.items || []).find((l) => l.title === title);
  if (found) return found.id;
  return (await api('POST', `${TASKS}/users/@me/lists`, { title })).id;
}

async function listAll(api, listId) {
  const out = [];
  let page = '';
  do {
    const d = await api('GET', `${TASKS}/lists/${listId}/tasks?showCompleted=true&showHidden=true&maxResults=100${page ? `&pageToken=${page}` : ''}`);
    out.push(...(d.items || []));
    page = d.nextPageToken;
  } while (page);
  return out;
}

function dueOf(deadline) {
  // Google 할 일의 due 는 날짜만 의미가 있다 (시간은 무시됨)
  return /^\d{4}-\d{2}-\d{2}$/.test(deadline || '') ? `${deadline}T00:00:00.000Z` : undefined;
}

/**
 * mine: 나에게 배정·수락된 사이 작업 [{ id, title, doneCriteria, status }]
 * links: { [saiTaskId]: googleTaskId | 'deleted' }
 * 돌려주는 값: { links, markDone: [saiTaskId], created, completedInGoogle, renamed }
 */
async function syncTasks(api, { listId, mine, links = {}, deadline, projectTitle }) {
  const remote = new Map((await listAll(api, listId)).map((t) => [t.id, t]));
  const next = { ...links };
  const res = { links: next, markDone: [], created: 0, completedInGoogle: 0, renamed: 0 };
  for (const t of mine) {
    const gid = next[t.id];
    if (gid === 'deleted') continue;
    const r = gid && remote.get(gid);
    if (gid && (!r || r.deleted)) {
      next[t.id] = 'deleted';
      continue;
    }
    if (!r) {
      if (t.status === 'done') continue;
      const created = await api('POST', `${TASKS}/lists/${listId}/tasks`, {
        title: t.title,
        notes: [t.doneCriteria && `완료 기준: ${t.doneCriteria}`, `사이 · ${projectTitle}`].filter(Boolean).join('\n'),
        due: dueOf(deadline),
      });
      next[t.id] = created.id;
      res.created++;
      continue;
    }
    if (r.status === 'completed' && t.status !== 'done') res.markDone.push(t.id);
    else if (t.status === 'done' && r.status !== 'completed') {
      await api('PATCH', `${TASKS}/lists/${listId}/tasks/${gid}`, { status: 'completed' });
      res.completedInGoogle++;
    }
    if (r.title !== t.title) {
      await api('PATCH', `${TASKS}/lists/${listId}/tasks/${gid}`, { title: t.title });
      res.renamed++;
    }
  }
  return res;
}

module.exports = { ensureList, syncTasks, listAll, TASKS };
