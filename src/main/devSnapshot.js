// 개발용: SAI_SNAPSHOT_DIR 환경 변수가 있으면 창을 캡처해 PNG로 저장하고 종료한다.
// 화면이 없는 환경(CI 등)에서 고양이와 대화창이 제대로 그려지는지 확인할 때 쓴다.
const fs = require('fs');
const path = require('path');

function runSnapshots(app, steps) {
  const dir = process.env.SAI_SNAPSHOT_DIR;
  if (!dir) return false;
  fs.mkdirSync(dir, { recursive: true });
  (async () => {
    for (const step of steps) {
      await new Promise((r) => setTimeout(r, step.wait || 800));
      try {
        if (step.action) await step.action();
        const win = step.window();
        if (step.capture !== false && win && !win.isDestroyed()) {
          await new Promise((r) => setTimeout(r, step.settle || 500));
          const img = await win.webContents.capturePage();
          fs.writeFileSync(path.join(dir, `${step.name}.png`), img.toPNG());
        }
      } catch (err) {
        console.error('snapshot failed', step.name, err);
      }
    }
    app.exit(0);
  })();
  return true;
}

module.exports = { runSnapshots };
