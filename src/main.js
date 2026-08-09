const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// 右クリックメニュー（Edgeのコンテキストメニュー）を無効化
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
  
  let processQueue = [];
  let activeCount = 0;
  let totalInCurrentBatch = 0;
  let processedInCurrentBatch = 0;
  let batchStartTime = 0;

  // Load saved settings
  const savedFolder = localStorage.getItem('defaultFolder') || '';
  const savedTemplate = localStorage.getItem('namingTemplate') || '$AUTHOR/$TITLE';
  const savedConcurrency = localStorage.getItem('concurrencyLimit') || '3';
  const savedDuplicateAction = localStorage.getItem('duplicateAction') || 'rename';
  const savedDeleteOriginal = localStorage.getItem('deleteOriginal') === 'true';
  const savedMinFiles = localStorage.getItem('notifyMinFiles') || '3';
  const savedMinTime = localStorage.getItem('notifyMinTime') || '30';
  
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
    localStorage.setItem('concurrencyLimit', e.target.value);
  });

  duplicateActionInput.addEventListener('change', (e) => {
    localStorage.setItem('duplicateAction', e.target.value);
  });

  deleteOriginalInput.addEventListener('change', (e) => {
    localStorage.setItem('deleteOriginal', e.target.checked);
  });

  notifyMinFilesInput.addEventListener('change', (e) => {
    localStorage.setItem('notifyMinFiles', e.target.value);
  });

  notifyMinTimeInput.addEventListener('change', (e) => {
    localStorage.setItem('notifyMinTime', e.target.value);
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
    
    // event.payload contains paths
    const rawFiles = event.payload.paths || event.payload; 
    
    if (rawFiles && rawFiles.length > 0) {
      const validFiles = [];
      let ignoredCount = 0;
      let nonIdCount = 0;

      rawFiles.forEach(f => {
        if (!f.toLowerCase().endsWith('.zip')) {
          ignoredCount++;
          return;
        }

        const fileName = f.split('\\').pop().split('/').pop();
        // Check for DLSite ID (e.g., RJ12345) or Fanza ID (e.g., d_123, abcd001)
        const idMatch = fileName.match(/RJ\d+/i) || fileName.match(/d_\d+|[a-z]+[0-9]{3,}/i);
        
        if (idMatch) {
          validFiles.push(f);
        } else {
          nonIdCount++;
        }
      });

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
        validFiles.forEach(filePath => {
          const fileName = filePath.split('\\').pop().split('/').pop();
          addTask(`⏳ 待機中: ${fileName}`);
        });
        
        // Add to queue
        processQueue.push(...validFiles);
        totalInCurrentBatch += validFiles.length;
        updateQueueUI();
        
        // Start processing if not already at limit
        pumpQueue();
      }
    }
  });
  
  function updateQueueUI() {
    if (totalInCurrentBatch > 0) {
      queueStatusContainer.style.display = 'block';
      queueProgressText.innerText = `${processedInCurrentBatch} / ${totalInCurrentBatch} 解凍済み`;
    } else {
      queueStatusContainer.style.display = 'none';
    }
  }
  
  async function pumpQueue() {
    const limit = parseInt(concurrencyLimitInput.value, 10) || 3;
    
    while (activeCount < limit && processQueue.length > 0) {
      const file = processQueue.shift();
      activeCount++;
      processSingleZip(file);
    }
  }
  
  async function processSingleZip(filePath) {
    const fileName = filePath.split('\\').pop().split('/').pop();
    const safeId = "progress-" + btoa(unescape(encodeURIComponent(fileName))).replace(/[^a-zA-Z0-9]/g, '');
    
    // Create progress bar UI
    const progressEl = document.createElement('div');
    progressEl.id = safeId;
    progressEl.innerHTML = `
      <div style="display: flex; justify-content: space-between; font-size: 13px; margin-bottom: 5px;">
        <span class="p-filename" style="overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 80%;">${fileName}</span>
        <span class="p-percent">0%</span>
      </div>
      <div style="width: 100%; height: 8px; background: var(--bg-color); border-radius: 4px; overflow: hidden; border: 1px solid var(--border-color);">
        <div class="p-bar" style="width: 0%; height: 100%; background: var(--accent-color); transition: width 0.1s linear;"></div>
      </div>
    `;
    progressBarsContainer.appendChild(progressEl);

    addTask(`▶️ 解凍開始: ${fileName}`);

    try {
      const result = await invoke('process_zip', { 
        filePathStr: filePath, 
        outputBase: defaultFolderInput.value, 
        template: namingTemplateInput.value,
        duplicateAction: duplicateActionInput.value,
        deleteOriginal: deleteOriginalInput.checked
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
          historyEl.style.cssText = 'display: flex; justify-content: space-between; align-items: center; padding: 10px; background: var(--bg-color); border-radius: 6px; border: 1px solid var(--border-color);';
          
          const infoDiv = document.createElement('div');
          infoDiv.style.cssText = 'display: flex; flex-direction: column; gap: 4px; max-width: 80%; overflow: hidden;';
          
          const titleSpan = document.createElement('span');
          titleSpan.style.cssText = 'font-size: 14px; font-weight: bold; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;';
          titleSpan.title = result.title || 'タイトル不明';
          titleSpan.textContent = result.title || 'タイトル不明';
          
          const fileSpan = document.createElement('span');
          fileSpan.style.cssText = 'font-size: 11px; color: var(--border-color); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;';
          fileSpan.title = fileName;
          fileSpan.textContent = fileName;
          
          infoDiv.appendChild(titleSpan);
          infoDiv.appendChild(fileSpan);
          
          const openBtn = document.createElement('button');
          openBtn.style.cssText = 'padding: 6px 12px; font-size: 12px; cursor: pointer; background: var(--accent-color); color: white; border: none; border-radius: 4px;';
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
          
          historyList.prepend(historyEl); // Add to top
        }
      }
    } catch (err) {
      addTask(`エラー (${fileName}): ${err}`);
    } finally {
      // Remove progress bar
      progressEl.remove();
      
      activeCount--;
      processedInCurrentBatch++;
      updateQueueUI();
      
      // Check if entire batch is done
      if (activeCount === 0 && processQueue.length === 0 && totalInCurrentBatch > 0) {
        const elapsedSeconds = (Date.now() - batchStartTime) / 1000;
        const minFiles = parseInt(notifyMinFilesInput.value, 10) || 3;
        const minTime = parseInt(notifyMinTimeInput.value, 10) || 30;

        if (totalInCurrentBatch >= minFiles || elapsedSeconds >= minTime) {
          checkAndSendNotification(`すべての解凍処理が完了しました (${totalInCurrentBatch}件 / ${Math.round(elapsedSeconds)}秒)`);
        }
        
        totalInCurrentBatch = 0;
        processedInCurrentBatch = 0;
        batchStartTime = 0;
        updateQueueUI();
        addTask("すべての解凍タスクが完了しました。");
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
    const { zip_filename, file, current, total } = event.payload;
    const safeId = "progress-" + btoa(unescape(encodeURIComponent(zip_filename))).replace(/[^a-zA-Z0-9]/g, '');
    const progressEl = document.getElementById(safeId);
    
    if (progressEl) {
      const percent = Math.floor((current / total) * 100);
      progressEl.querySelector('.p-bar').style.width = `${percent}%`;
      progressEl.querySelector('.p-percent').innerText = `${percent}%`;
      progressEl.querySelector('.p-filename').innerText = file;
    }
  });

  listen('tauri://drag-enter', () => {
    dropZone.classList.add('drag-over');
  });

  listen('tauri://drag-leave', () => {
    dropZone.classList.remove('drag-over');
  });

  function addTask(message) {
    if (taskQueue.children.length === 1 && taskQueue.children[0].innerText.includes('タスクはありません')) {
      taskQueue.innerHTML = '';
    }
    
    const taskEl = document.createElement('div');
    taskEl.className = 'task-item';
    taskEl.innerText = message;
    taskQueue.prepend(taskEl);
  }
});
