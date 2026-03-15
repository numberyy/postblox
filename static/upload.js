(function() {
  'use strict';

  function initUpload(zone) {
    var inboxId = zone.dataset.inboxId;
    if (!inboxId) return;

    var input = zone.querySelector('input[type="file"]');
    var status = zone.querySelector('.upload-status');

    zone.addEventListener('dragover', function(e) {
      e.preventDefault();
      zone.classList.add('drop-active');
    });

    zone.addEventListener('dragleave', function() {
      zone.classList.remove('drop-active');
    });

    zone.addEventListener('drop', function(e) {
      e.preventDefault();
      zone.classList.remove('drop-active');
      if (e.dataTransfer.files.length) upload(e.dataTransfer.files);
    });

    if (input) {
      input.addEventListener('change', function() {
        if (input.files.length) upload(input.files);
      });
    }

    function upload(files) {
      var fd = new FormData();
      for (var i = 0; i < files.length; i++) fd.append('files', files[i]);

      if (status) {
        status.textContent = 'Uploading...';
        status.className = 'upload-status';
      }

      fetch('/inboxes/' + inboxId + '/messages/upload', {
        method: 'POST',
        body: fd,
      }).then(function(r) {
        if (!r.ok) throw new Error(r.status + ' ' + r.statusText);
        return r.json();
      }).then(function() {
        if (status) {
          status.textContent = 'Upload complete';
          status.className = 'upload-status upload-ok';
        }
        if (input) input.value = '';
      }).catch(function(err) {
        if (status) {
          status.textContent = 'Upload failed: ' + err.message;
          status.className = 'upload-status upload-err';
        }
      });
    }
  }

  document.addEventListener('DOMContentLoaded', function() {
    var zones = document.querySelectorAll('.drop-zone');
    for (var i = 0; i < zones.length; i++) initUpload(zones[i]);
  });
})();
