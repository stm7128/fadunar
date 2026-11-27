const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// Disable default browser context menu
document.addEventListener('contextmenu', e => e.preventDefault());

document.addEventListener('DOMContentLoaded', () => {
  const dropZone = document.getElementById('drop-zone');
  const btnSelectFolder = document.getElementById('btn-select-folder');
  const defaultFolderInput = document.getElementById('default-folder');
  const namingTemplateInput = document.getElementById('naming-template');
  const concurrencyLimitInput = document.getElementById('concurrency-limit');
  const duplicateActionInput = document.getElementById('duplicate-action');
  const deleteOriginalInput = document.getElementById('delete-original');
  const notifyMinFilesInput = document.getElementById('notify-min-files');
  const notifyMinTimeInput = document.getElementById('notify-min-time');
  const taskQueue = document.getElementById('task-queue');

  const queueStatusContainer = document.getElementById('queue-status-container');
  const queueProgressText = document.getElementById('queue-progress-text');
  const progressBarsContainer = document.getElementById('progress-bars-container');

  const historyList = document.getElementById('history-list');

  const MAX_HISTORY_ITEMS = 100;
  const MAX_LOG_ITEMS = 200;

  let processQueue = [];
  let activeCount = 0;
  let totalInCurrentBatch = 0;
  let processedInCurrentBatch = 0;
  let batchStartTime = 0;
  let taskSequence = 0;
  const activeProgressBars = new Map();

  function clampInt(value, fallback, min, max = Number.MAX_SAFE_INTEGER) {
    const parsed = parseInt(value, 10);
    if (!Number.isFinite(parsed)) {
      return fallback;
    }
    return Math.min(max, Math.max(min, parsed));
  }

  function nextTaskId() {
    if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
      return crypto.randomUUID();
    }
    taskSequence += 1;
    return `task-${Date.now()}-${taskSequence}`;
  }

  // Load saved settings
  const savedFolder = localStorage.getItem('defaultFolder') || '';
  const savedTemplate = localStorage.getItem('namingTemplate') || '$AUTHOR/$TITLE';
  const savedConcurrency = String(clampInt(localStorage.getItem('concurrencyLimit'), 3, 1, 20));
  const savedDuplicateAction = localStorage.getItem('duplicateAction') || 'rename';
  const savedDeleteOriginal = localStorage.getItem('deleteOriginal') === 'true';
  const savedMinFiles = String(clampInt(localStorage.getItem('notifyMinFiles'), 3, 1));
  const savedMinTime = String(clampInt(localStorage.getItem('notifyMinTime'), 30, 1));

  defaultFolderInput.value = savedFolder;
  namingTemplateInput.value = savedTemplate;
  concurrencyLimitInput.value = savedConcurrency;
  duplicateActionInput.value = savedDuplicateAction;
  deleteOriginalInput.checked = savedDeleteOriginal;
  notifyMinFilesInput.value = savedMinFiles;
  notifyMinTimeInput.value = savedMinTime;

  namingTemplateInput.addEventListener('change', (e) => {
    localStorage.setItem('namingTemplate', e.target.value);
  });

  concurrencyLimitInput.addEventListener('change', (e) => {
    const clamped = String(clampInt(e.target.value, 3, 1, 20));
    e.target.value = clamped;
    localStorage.setItem('concurrencyLimit', clamped);
  });

  duplicateActionInput.addEventListener('change', (e) => {
    localStorage.setItem('duplicateAction', e.target.value);
  });

  deleteOriginalInput.addEventListener('change', (e) => {
    localStorage.setItem('deleteOriginal', e.target.checked);
  });

  notifyMinFilesInput.addEventListener('change', (e) => {
    const clamped = String(clampInt(e.target.value, 3, 1));
    e.target.value = clamped;
    localStorage.setItem('notifyMinFiles', clamped);
  });

  notifyMinTimeInput.addEventListener('change', (e) => {
    const clamped = String(clampInt(e.target.value, 30, 1));
    e.target.value = clamped;
    localStorage.setItem('notifyMinTime', clamped);
  });

  const btnResetSettings = document.getElementById('btn-reset-settings');
  if (btnResetSettings) {
    btnResetSettings.addEventListener('click', () => {
      if (confirm('保存されている設定（出力先、命名規則、同時解凍数など）を全て初期化しますか？')) {
        localStorage.clear();
        location.reload();
      }
    });
  }

  btnSelectFolder.addEventListener('click', async () => {
    try {
      const selected = await invoke('select_directory');
      if (selected) {
        defaultFolderInput.value = selected;
        localStorage.setItem('defaultFolder', selected);
      }
    } catch (e) {
      console.error(e);
    }
  });

  // Listen to Tauri's native drop event
  listen('tauri://drag-drop', async (event) => {
    dropZone.classList.remove('drag-over');

    const rawFiles = event.payload.paths || event.payload;
    if (!rawFiles || rawFiles.length === 0) {
      return;
    }

    if (!defaultFolderInput.value.trim()) {
      addTask('⚠️ デフォルト保存先フォルダを先に選択してください');
      return;
    }

    let classification;
    try {
      classification = await invoke('classify_dropped_files', { paths: rawFiles });
    } catch (err) {
      addTask(`エラー: ファイル判定に失敗しました (${err})`);
      return;
    }

    const {
      valid: validFiles,
      ignored_non_zip: ignoredCount,
      ignored_unknown_id: nonIdCount,
    } = classification;

    if (ignoredCount > 0) {
      addTask(`⚠️ ${ignoredCount}件のZIP以外のファイルを無視しました`);
    }
    if (nonIdCount > 0) {
      addTask(`⚠️ ${nonIdCount}件の非対応ZIP(ID不明)を無視しました`);
    }

    if (validFiles.length > 0) {
      if (totalInCurrentBatch === 0) {
        batchStartTime = Date.now();
      }

      addTask(`✅ ${validFiles.length}件のZIPファイルをキューに追加しました`);
      validFiles.forEach((filePath) => {
        const fileName = filePath.split('\\').pop().split('/').pop();
        addTask(`⏳ 待機中: ${fileName}`);
      });

      processQueue.push(...validFiles);
      totalInCurrentBatch += validFiles.length;
      updateQueueUI();

      pumpQueue();
    }
  });

  function updateQueueUI() {
    if (totalInCurrentBatch > 0) {
      queueStatusContainer.style.display = 'block';
      queueProgressText.textContent = `${processedInCurrentBatch} / ${totalInCurrentBatch} 解凍済み`;
    } else {
      queueStatusContainer.style.display = 'none';
    }
  }

  async function pumpQueue() {
    const limit = clampInt(concurrencyLimitInput.value, 3, 1, 20);

    while (activeCount < limit && processQueue.length > 0) {
      const file = processQueue.shift();
      activeCount++;
      processSingleZip(file);
    }
  }

  function createProgressElement(fileName) {
    const progressEl = document.createElement('div');

    const header = document.createElement('div');
    header.className = 'progress-header';

    const nameSpan = document.createElement('span');
    nameSpan.className = 'p-filename progress-filename';
    nameSpan.textContent = fileName;

    const percentSpan = document.createElement('span');
    percentSpan.className = 'p-percent';
    percentSpan.textContent = '0%';

    header.appendChild(nameSpan);
    header.appendChild(percentSpan);

    const track = document.createElement('div');
    track.className = 'progress-track';

    const bar = document.createElement('div');
    bar.className = 'p-bar';
    track.appendChild(bar);

    progressEl.appendChild(header);
    progressEl.appendChild(track);
    return progressEl;
  }

  async function processSingleZip(filePath) {
    const fileName = filePath.split('\\').pop().split('/').pop();
    const taskId = nextTaskId();

    const progressEl = createProgressElement(fileName);
    activeProgressBars.set(taskId, progressEl);
    progressBarsContainer.appendChild(progressEl);

    addTask(`▶️ 解凍開始: ${fileName}`);

    try {
      const result = await invoke('process_zip', {
        taskId,
        filePathStr: filePath,
        outputBase: defaultFolderInput.value,
        template: namingTemplateInput.value,
        duplicateAction: duplicateActionInput.value,
        deleteOriginal: deleteOriginalInput.checked,
      });

      if (result.success) {
        addTask(result.message);

        // Add to history
        if (!result.skipped && result.output_path) {
          const historyEmpty = document.getElementById('history-empty');
          if (historyEmpty) {
            historyEmpty.remove();
          }

          const historyEl = document.createElement('div');
          historyEl.className = 'history-item';

          const infoDiv = document.createElement('div');
          infoDiv.className = 'history-info';

          const titleSpan = document.createElement('span');
          titleSpan.className = 'history-title';
          titleSpan.title = result.title || 'タイトル不明';
          titleSpan.textContent = result.title || 'タイトル不明';

          const fileSpan = document.createElement('span');
          fileSpan.className = 'history-filename';
          fileSpan.title = fileName;
          fileSpan.textContent = fileName;

          infoDiv.appendChild(titleSpan);
          infoDiv.appendChild(fileSpan);

          const openBtn = document.createElement('button');
          openBtn.type = 'button';
          openBtn.className = 'btn-open-folder';
          openBtn.textContent = 'フォルダを開く';
          openBtn.addEventListener('click', async () => {
            try {
              await invoke('open_folder', { path: result.output_path });
            } catch (err) {
              addTask(`フォルダを開けませんでした: ${err}`);
            }
          });

          historyEl.appendChild(infoDiv);
          historyEl.appendChild(openBtn);

          historyList.prepend(historyEl);
          while (historyList.children.length > MAX_HISTORY_ITEMS) {
            historyList.lastElementChild.remove();
          }
        }
      }
    } catch (err) {
      addTask(`エラー (${fileName}): ${err}`);
    } finally {
      activeProgressBars.delete(taskId);
      progressEl.remove();

      activeCount--;
      processedInCurrentBatch++;
      updateQueueUI();

      // Check if entire batch is done
      if (activeCount === 0 && processQueue.length === 0 && totalInCurrentBatch > 0) {
        const elapsedSeconds = (Date.now() - batchStartTime) / 1000;
        const minFiles = clampInt(notifyMinFilesInput.value, 3, 1);
        const minTime = clampInt(notifyMinTimeInput.value, 30, 1);

        if (totalInCurrentBatch >= minFiles || elapsedSeconds >= minTime) {
          checkAndSendNotification(
            `すべての解凍処理が完了しました (${totalInCurrentBatch}件 / ${Math.round(elapsedSeconds)}秒)`
          );
        }

        totalInCurrentBatch = 0;
        processedInCurrentBatch = 0;
        batchStartTime = 0;
        updateQueueUI();
        addTask('すべての解凍タスクが完了しました。');
      } else {
        pumpQueue();
      }
    }
  }

  async function checkAndSendNotification(body) {
    try {
      let permissionGranted = await invoke('plugin:notification|is_permission_granted');
      if (!permissionGranted) {
        const permission = await invoke('plugin:notification|request_permission');
        permissionGranted = permission === 'granted';
      }
      if (permissionGranted) {
        await invoke('plugin:notification|notify', { options: { title: 'Fadunar', body } });
      }
    } catch (e) {
      console.error('Notification error:', e);
    }
  }

  // Listen to progress updates
  listen('extract-progress', (event) => {
    const { task_id: taskId, file, current, total } = event.payload;
    const progressEl = activeProgressBars.get(taskId);

    if (progressEl) {
      const percent = total > 0 ? Math.min(100, Math.floor((current / total) * 100)) : 0;
      progressEl.querySelector('.p-bar').style.width = `${percent}%`;
      progressEl.querySelector('.p-percent').textContent = `${percent}%`;
      progressEl.querySelector('.p-filename').textContent = file;
    }
  });

  listen('tauri://drag-enter', () => {
    dropZone.classList.add('drag-over');
  });

  listen('tauri://drag-leave', () => {
    dropZone.classList.remove('drag-over');
  });

  function addTask(message) {
    if (taskQueue.children.length === 1 && taskQueue.children[0].textContent.includes('タスクはありません')) {
      taskQueue.replaceChildren();
    }

    const taskEl = document.createElement('div');
    taskEl.className = 'task-item';
    taskEl.textContent = message;
    taskQueue.prepend(taskEl);
    while (taskQueue.children.length > MAX_LOG_ITEMS) {
      taskQueue.lastElementChild.remove();
    }
  }
});
