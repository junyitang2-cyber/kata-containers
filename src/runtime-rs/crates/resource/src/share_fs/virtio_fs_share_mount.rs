// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use agent::Storage;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use kata_types::k8s::is_watchable_mount;
use kata_types::mount;
use nix::sys::stat::stat;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const WATCHABLE_PATH_NAME: &str = "watchable";
const WATCHABLE_BIND_DEV_TYPE: &str = "watchable-bind";
const EPHEMERAL_PATH: &str = "/run/kata-containers/sandbox/ephemeral";

use super::{
    utils, ShareFsMount, ShareFsMountResult, ShareFsRootfsConfig, ShareFsVolumeConfig,
    KATA_GUEST_SHARE_DIR, PASSTHROUGH_FS_DIR,
};

pub struct VirtiofsShareMount {
    id: String,
}

impl VirtiofsShareMount {
    pub fn new(id: &str) -> Self {
        Self { id: id.to_string() }
    }
}

#[async_trait]
impl ShareFsMount for VirtiofsShareMount {
    async fn share_rootfs(&self, config: ShareFsRootfsConfig) -> Result<ShareFsMountResult> {
        // TODO: select virtiofs or support nydus
        let guest_path = utils::share_to_guest(
            &config.source,
            &config.target,
            &self.id,
            &config.cid,
            config.readonly,
            false,
            config.is_rafs,
        )
        .context("share to guest")?;
        Ok(ShareFsMountResult {
            guest_path,
            storages: vec![],
        })
    }

    async fn share_volume(&self, config: ShareFsVolumeConfig) -> Result<ShareFsMountResult> {
        let mut guest_path = utils::share_to_guest(
            &config.source,
            &config.target,
            &self.id,
            &config.cid,
            config.readonly,
            true,
            config.is_rafs,
        )
        .context("share to guest")?;

        // watchable mounts
        if is_watchable_mount(&config.source) {
            // Create path in shared directory for creating watchable mount:
            let host_rw_path = utils::get_host_rw_shared_path(&self.id);

            // "/run/kata-containers/shared/sandboxes/$sid/rw/passthrough/watchable"
            let watchable_host_path = Path::new(&host_rw_path)
                .join(PASSTHROUGH_FS_DIR)
                .join(WATCHABLE_PATH_NAME);

            fs::create_dir_all(&watchable_host_path).context(format!(
                "unable to create watchable path: {:?}",
                &watchable_host_path,
            ))?;

            fs::set_permissions(watchable_host_path, fs::Permissions::from_mode(0o750))?;

            // path: /run/kata-containers/shared/containers/passthrough/watchable/config-map-name
            let file_name = Path::new(&guest_path)
                .file_name()
                .context("get file name from guest path")?;
            let watchable_guest_mount = Path::new(KATA_GUEST_SHARE_DIR)
                .join(PASSTHROUGH_FS_DIR)
                .join(WATCHABLE_PATH_NAME)
                .join(file_name)
                .into_os_string()
                .into_string()
                .map_err(|e| anyhow!("failed to get watchable guest mount path {:?}", e))?;

            let watchable_storage: Storage = Storage {
                driver: String::from(WATCHABLE_BIND_DEV_TYPE),
                driver_options: Vec::new(),
                source: guest_path,
                fs_type: String::from("bind"),
                fs_group: None,
                options: config.mount_options,
                mount_point: watchable_guest_mount.clone(),
            };

            // Update the guest_path, in order to identify what will
            // change in the OCI spec.
            guest_path = watchable_guest_mount;

            let storages = vec![watchable_storage];

            return Ok(ShareFsMountResult {
                guest_path,
                storages,
            });
        } else if config.mount.r#type == mount::KATA_EPHEMERAL_VOLUME_TYPE {
            // refer to the golang `handleEphemeralStorage` code at
            // https://github.com/kata-containers/kata-containers/blob/9516286f6dd5cfd6b138810e5d7c9e01cf6fc043/src/runtime/virtcontainers/kata_agent.go#L1354

            let source = &config.mount.source;
            let file_stat =
                stat(Path::new(source)).with_context(|| format!("mount source {}", source))?;

            // if volume's gid isn't root group(default group), this means there's
            // an specific fsGroup is set on this local volume, then it should pass
            // to guest.
            let dir_options = if file_stat.st_gid != 0 {
                vec![format!("fsgid={}", file_stat.st_gid)]
            } else {
                vec![]
            };

            let file_name = Path::new(source)
                .file_name()
                .context("get file name from mount.source")?;
            let source = Path::new(EPHEMERAL_PATH)
                .join(file_name)
                .into_os_string()
                .into_string()
                .map_err(|e| anyhow!("failed to get ephemeral path {:?}", e))?;

            // Create a storage struct so that kata agent is able to create
            // tmpfs backed volume inside the VM
            let ephemeral_storage = agent::Storage {
                driver: String::from(mount::KATA_EPHEMERAL_VOLUME_TYPE),
                driver_options: Vec::new(),
                source: String::from("tmpfs"),
                fs_type: String::from("tmpfs"),
                fs_group: None,
                options: dir_options,
                mount_point: source.clone(),
            };

            guest_path = source;
            let storages = vec![ephemeral_storage];

            return Ok(ShareFsMountResult {
                guest_path,
                storages,
            });
        }

        Ok(ShareFsMountResult {
            guest_path,
            storages: vec![],
        })
    }
}
