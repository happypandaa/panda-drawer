use super::*;
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Point {
    Bound,
    IntentPublished,
    OldHeld,
    PayloadPublished,
    OldAtStage,
    DecisionPublished,
    StageForgotten,
    IntentRetired,
    ControlsRetired,
}

fn authority(owner: &WebProjectOwnership, id: &str, plan: &Plan) -> Result<()> {
    let record = owner.read(id)?;
    let stage = record
        .export_stages
        .get(&plan.task)
        .ok_or_else(|| failure("交付临时文件未预先登记。"))?;
    if stage.path != plan.stage
        || !stage
            .parent_identity
            .matches_captured(&plan.parent_identity)
        || !stage
            .identity
            .as_ref()
            .is_some_and(|identity| identity.matches_captured(&plan.payload))
        || !same(stage.overwrite_identity.as_ref(), plan.selected.as_ref())
    {
        return Err(failure("交付原始授权身份变化，已保留现场。"));
    }
    Ok(())
}
fn scope(
    owner: &WebProjectOwnership,
    id: &str,
    task: &str,
    binding: &Binding,
) -> Result<(native::PinnedDirectory, native::PinnedDirectory, PathBuf)> {
    validate_binding(task, binding)?;
    let owner_pin = pin(&owner.directory(id)?, &binding.owner_identity)?;
    let path = directory(owner, id, task)?;
    let transaction_pin = pin(&path, &binding.directory_identity)?;
    Ok((owner_pin, transaction_pin, path))
}

pub(super) fn publish(
    owner: &WebProjectOwnership,
    id: &str,
    plan: Plan,
    mut observe: impl FnMut(Point) -> Result<()>,
) -> Result<()> {
    plan.validate()?;
    authority(owner, id, &plan)?;
    let parent = pin(plan.stage.parent().unwrap(), &plan.parent_identity)?;
    if !same(Some(&plan.payload), file_identity(&plan.stage)?.as_ref())
        || !same(
            plan.selected.as_ref(),
            file_identity(&plan.destination)?.as_ref(),
        )
        || file_identity(&plan.held())?.is_some()
    {
        return Err(failure("交付发布前名字或身份变化，未覆盖任何文件。"));
    }
    let owner_path = owner.directory(id)?;
    let owner_pin = native::pin_directory(&owner_path)?;
    let container = owner_path.join(CONTAINER);
    match native::create_private_directory(&container) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(cause(error)),
    }
    let _container = native::pin_directory(&container)?;
    let path = directory(owner, id, &plan.task)?;
    native::create_private_directory(&path)?;
    let mut binding = Binding {
        owner_identity: PersistedIdentity::from_file(&owner_pin.file).map_err(cause)?,
        directory_identity: PersistedIdentity::capture(&path).map_err(cause)?,
        plan,
        controls: None,
        completed: false,
    };
    owner.update(id, false, |record| {
        if record.native_deliveries.contains_key(&binding.plan.task) {
            return Err(failure("交付任务已有事务，未重复接纳。"));
        }
        record
            .native_deliveries
            .insert(binding.plan.task.clone(), binding.clone());
        Ok(())
    })?;
    let _transaction = pin(&path, &binding.directory_identity)?;
    let mut intent = native::create_private_file_for_publication(&path.join(INTENT_PREPARING))?;
    let mut decision = native::create_private_file_for_publication(&path.join(DECISION_PREPARING))?;
    let intent_id = PersistedIdentity::from_file(&intent).map_err(cause)?;
    let decision_id = PersistedIdentity::from_file(&decision).map_err(cause)?;
    let intent_contents = intent_bytes(id, &binding)?;
    binding.controls = Some(Controls {
        intent_identity: intent_id,
        intent_sha256: digest(&intent_contents),
        decision_identity: decision_id,
        decision_sha256: String::new(),
    });
    let decision_contents = decision_bytes(id, &binding)?;
    binding.controls.as_mut().unwrap().decision_sha256 = digest(&decision_contents);
    if intent_contents.len() as u64 > MAX_CONTROL || decision_contents.len() as u64 > MAX_CONTROL {
        return Err(failure("交付控制材料超出大小限制。"));
    }
    // Exact new HANDLE identities and expected bytes precede every control
    // write. A published intent can only come from these fully flushed files.
    owner.update(id, false, |record| {
        let current = record
            .native_deliveries
            .get_mut(&binding.plan.task)
            .ok_or_else(|| failure("交付事务归属缺失。"))?;
        if current.controls.is_some()
            || !current
                .directory_identity
                .matches_captured(&binding.directory_identity)
        {
            return Err(failure("交付事务归属变化。"));
        }
        current.controls = binding.controls.clone();
        Ok(())
    })?;
    intent.write_all(&intent_contents)?;
    intent.sync_all()?;
    decision.write_all(&decision_contents)?;
    decision.sync_all()?;
    drop(decision);
    observe(Point::Bound)?;
    native::rename_private_file_handle(&intent, &path.join(INTENT), false)?;
    drop(intent);
    observe(Point::IntentPublished)?;
    let controls = binding.controls.as_ref().unwrap();
    let active = read_control(
        &path.join(INTENT),
        &controls.intent_identity,
        &controls.intent_sha256,
    )?
    .ok_or_else(|| failure("交付意图发布后消失。"))?;
    apply(&binding, &mut observe)?;
    confirm_and_decide(owner, id, &binding, &path, &mut observe)?;
    drop(active);
    drop(_transaction);
    drop(_container);
    drop(owner_pin);
    drop(parent);
    finish(owner, id, &binding, &mut observe)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Prepared,
    Held,
    Installed,
    Swapped,
}
fn state(plan: &Plan) -> Result<State> {
    let source = file_identity(&plan.stage)?;
    let destination = file_identity(&plan.destination)?;
    let held = file_identity(&plan.held())?;
    if same(Some(&plan.payload), source.as_ref())
        && same(plan.selected.as_ref(), destination.as_ref())
        && held.is_none()
    {
        return Ok(State::Prepared);
    }
    if let Some(selected) = &plan.selected {
        if same(Some(&plan.payload), source.as_ref())
            && destination.is_none()
            && same(Some(selected), held.as_ref())
        {
            return Ok(State::Held);
        }
        if source.is_none()
            && same(Some(&plan.payload), destination.as_ref())
            && same(Some(selected), held.as_ref())
        {
            return Ok(State::Installed);
        }
        if same(Some(selected), source.as_ref())
            && same(Some(&plan.payload), destination.as_ref())
            && held.is_none()
        {
            return Ok(State::Swapped);
        }
    } else if source.is_none() && same(Some(&plan.payload), destination.as_ref()) && held.is_none()
    {
        return Ok(State::Swapped);
    }
    Err(
        failure("交付三个名字的完整身份不一致；原件、临时材料和事务均已保留。")
            .detail("stagePath", json!(plan.stage))
            .detail("destinationPath", json!(plan.destination))
            .detail("heldPath", json!(plan.held())),
    )
}
fn apply(binding: &Binding, observe: &mut impl FnMut(Point) -> Result<()>) -> Result<()> {
    let plan = &binding.plan;
    let _parent = pin(plan.stage.parent().unwrap(), &plan.parent_identity)?;
    if state(plan)? == State::Prepared {
        if let Some(selected) = &plan.selected {
            move_file(&plan.destination, &plan.held(), selected)?;
            observe(Point::OldHeld)?;
        }
        move_file(&plan.stage, &plan.destination, &plan.payload)?;
        observe(Point::PayloadPublished)?;
    } else if state(plan)? == State::Held {
        move_file(&plan.stage, &plan.destination, &plan.payload)?;
        observe(Point::PayloadPublished)?;
    }
    if state(plan)? == State::Installed {
        move_file(&plan.held(), &plan.stage, plan.selected.as_ref().unwrap())?;
        observe(Point::OldAtStage)?;
    }
    if state(plan)? != State::Swapped {
        return Err(failure("交付发布尚未核实。"));
    }
    Ok(())
}
fn confirm_and_decide(
    owner: &WebProjectOwnership,
    id: &str,
    binding: &Binding,
    path: &Path,
    observe: &mut impl FnMut(Point) -> Result<()>,
) -> Result<()> {
    let controls = binding.controls.as_ref().unwrap();
    if optional_control(
        &path.join(COMMITTED),
        &controls.decision_identity,
        &controls.decision_sha256,
    )? {
        return Ok(());
    }
    if state(&binding.plan)? != State::Swapped {
        return Err(failure("交付尚未确认，未发布完成决定。"));
    }
    if let Some(selected) = &binding.plan.selected {
        owner.verify_export_stage_overwrite(
            id,
            &binding.plan.task,
            &binding.plan.stage,
            &binding.plan.payload,
            &binding.plan.destination,
            selected,
        )?;
    }
    let prepared = read_control(
        &path.join(DECISION_PREPARING),
        &controls.decision_identity,
        &controls.decision_sha256,
    )?
    .ok_or_else(|| failure("交付完成决定的原始已刷新文件缺失。"))?;
    // Only this original fully flushed file may become the decision. No new
    // JSON is synthesized to infer completion after an interrupted operation.
    move_control(
        &path.join(DECISION_PREPARING),
        &path.join(COMMITTED),
        &controls.decision_identity,
    )?;
    drop(prepared);
    observe(Point::DecisionPublished)
}

pub(super) fn recover(
    owner: &WebProjectOwnership,
    id: &str,
    task: &str,
    binding: &Binding,
) -> Result<()> {
    validate_binding(task, binding)?;
    let path = directory(owner, id, task)?;
    let owner_pin = pin(&owner.directory(id)?, &binding.owner_identity)?;
    if binding
        .directory_identity
        .absent_on_expected_volume(&path)
        .map_err(cause)?
    {
        // The binding survives the last directory deletion. No external name
        // is ever replayed after retiring that exact transaction directory.
        drop(owner_pin);
        if !binding.completed {
            authority(owner, id, &binding.plan)?;
            let _parent = pin(
                binding.plan.stage.parent().unwrap(),
                &binding.plan.parent_identity,
            )?;
            if state(&binding.plan)? != State::Prepared {
                return Err(failure("交付事务目录缺失但外部名字尚未归还，已保留。"));
            }
        }
        return forget_binding(owner, id, task, binding.completed);
    }
    if binding.completed {
        drop(owner_pin);
        return retire(owner, id, binding, true, &mut |_| Ok(()));
    }
    let (_owner, transaction, path) = scope(owner, id, task, binding)?;
    let Some(controls) = &binding.controls else {
        if fs::read_dir(&path)?.next().is_some() {
            return Err(failure("未绑定的交付控制材料已保留。"));
        }
        drop(transaction);
        drop(_owner);
        drop(owner_pin);
        remove_name(&path, &binding.directory_identity, true)?;
        return forget_binding(owner, id, task, false);
    };
    let committed = read_control(
        &path.join(COMMITTED),
        &controls.decision_identity,
        &controls.decision_sha256,
    )?;
    if committed.is_some() {
        drop(committed);
        drop(transaction);
        drop(_owner);
        drop(owner_pin);
        return finish(owner, id, binding, &mut |_| Ok(()));
    }
    let active = read_control(
        &path.join(INTENT),
        &controls.intent_identity,
        &controls.intent_sha256,
    )?;
    let Some(active) = active else {
        // No published intent means no external rename could have started.
        // Registered original control IDs may be cleaned even after a partial
        // private write; unknown names or replacements are never adopted.
        authority(owner, id, &binding.plan)?;
        let _parent = pin(
            binding.plan.stage.parent().unwrap(),
            &binding.plan.parent_identity,
        )?;
        if state(&binding.plan)? != State::Prepared {
            return Err(failure("交付意图缺失且外部名字曾移动，保留所有材料。"));
        }
        drop(_parent);
        drop(transaction);
        drop(_owner);
        drop(owner_pin);
        return retire(owner, id, binding, false, &mut |_| Ok(()));
    };
    let _parent = pin(
        binding.plan.stage.parent().unwrap(),
        &binding.plan.parent_identity,
    )?;
    authority(owner, id, &binding.plan)?;
    match state(&binding.plan)? {
        State::Prepared => {
            drop(active);
            drop(transaction);
            drop(_owner);
            drop(owner_pin);
            drop(_parent);
            // An unpublished external delivery is aborted, not newly delivered
            // because the user asked to delete a project after restarting.
            retire(owner, id, binding, false, &mut |_| Ok(()))
        }
        State::Held => {
            move_file(
                &binding.plan.held(),
                &binding.plan.destination,
                binding.plan.selected.as_ref().unwrap(),
            )?;
            if state(&binding.plan)? != State::Prepared {
                return Err(failure("旧用户文件归还尚未确认，已保留。"));
            }
            drop(active);
            drop(transaction);
            drop(_owner);
            drop(owner_pin);
            drop(_parent);
            retire(owner, id, binding, false, &mut |_| Ok(()))
        }
        State::Installed | State::Swapped => {
            apply(binding, &mut |_| Ok(()))?;
            confirm_and_decide(owner, id, binding, &path, &mut |_| Ok(()))?;
            drop(active);
            drop(transaction);
            drop(_owner);
            drop(owner_pin);
            drop(_parent);
            finish(owner, id, binding, &mut |_| Ok(()))
        }
    }
}

fn finish(
    owner: &WebProjectOwnership,
    id: &str,
    binding: &Binding,
    observe: &mut impl FnMut(Point) -> Result<()>,
) -> Result<()> {
    let (_owner, _transaction, path) = scope(owner, id, &binding.plan.task, binding)?;
    let controls = binding.controls.as_ref().unwrap();
    let decision = read_control(
        &path.join(COMMITTED),
        &controls.decision_identity,
        &controls.decision_sha256,
    )?
    .ok_or_else(|| failure("交付完成回执缺失，未清理旧用户文件。"))?;
    let record = owner.read(id)?;
    if let Some(stage) = record.export_stages.get(&binding.plan.task) {
        authority(owner, id, &binding.plan)?;
        if let Some(selected) = &binding.plan.selected {
            let _parent = pin(
                binding.plan.stage.parent().unwrap(),
                &binding.plan.parent_identity,
            )?;
            if !stage.overwrite_verified {
                owner.update(id, false, |record| {
                    let stage = record
                        .export_stages
                        .get_mut(&binding.plan.task)
                        .ok_or_else(|| failure("已完成交付的临时归属缺失。"))?;
                    if !stage
                        .identity
                        .as_ref()
                        .is_some_and(|identity| identity.matches_captured(&binding.plan.payload))
                        || !stage
                            .overwrite_identity
                            .as_ref()
                            .is_some_and(|identity| identity.matches_captured(selected))
                    {
                        return Err(failure("已完成交付的原覆盖授权变化。"));
                    }
                    stage.overwrite_verified = true;
                    Ok(())
                })?;
            }
            crate::web_file_deletion::remove_owned_file(owner, id, &binding.plan.stage, selected)?;
        }
        // The final path is never registered as owned. A user may already have
        // moved or edited that completed delivery; do not read or reclaim it.
        owner.forget_export_stage(id, &binding.plan.task)?;
    }
    observe(Point::StageForgotten)?;
    owner.update(id, false, |record| {
        let current = record
            .native_deliveries
            .get_mut(&binding.plan.task)
            .ok_or_else(|| failure("交付事务归属缺失。"))?;
        if !current
            .directory_identity
            .matches_captured(&binding.directory_identity)
        {
            return Err(failure("交付事务归属变化。"));
        }
        current.completed = true;
        Ok(())
    })?;
    drop(decision);
    drop(_transaction);
    drop(_owner);
    retire(owner, id, binding, true, observe)
}
fn forget_binding(
    owner: &WebProjectOwnership,
    id: &str,
    task: &str,
    completed: bool,
) -> Result<()> {
    owner.update(id, false, |record| {
        record.native_deliveries.remove(task);
        Ok(())
    })?;
    if completed {
        owner.forget_native_copy_task(id, task)?;
    }
    Ok(())
}
fn retire(
    owner: &WebProjectOwnership,
    id: &str,
    binding: &Binding,
    completed: bool,
    observe: &mut impl FnMut(Point) -> Result<()>,
) -> Result<()> {
    let (_owner, transaction, path) = scope(owner, id, &binding.plan.task, binding)?;
    let controls = binding.controls.as_ref();
    // Validate every entry before unlinking any control file. Only this
    // transaction's recorded original names and IDs may be retired.
    let mut files = Vec::new();
    for entry in fs::read_dir(&path)? {
        let entry = entry?;
        let name = entry.file_name();
        let (expected, hash) = match (name.to_str(), controls) {
            (Some(INTENT | INTENT_PREPARING), Some(c)) => (&c.intent_identity, &c.intent_sha256),
            (Some(COMMITTED | DECISION_PREPARING), Some(c)) => {
                (&c.decision_identity, &c.decision_sha256)
            }
            _ => return Err(failure("交付事务目录出现未知材料，已保留。")),
        };
        let file = native::open_read_file(&entry.path())?;
        native::verify_private(&file)?;
        if file.metadata()?.len() > MAX_CONTROL
            || !expected.matches_open_runtime(&file).map_err(cause)?
        {
            return Err(failure("交付控制材料被替换，已保留。"));
        }
        drop(file);
        if completed || name == INTENT || name == COMMITTED {
            read_control(&entry.path(), expected, hash)?;
        }
        files.push((entry.path(), expected.clone()));
    }
    // Keep the published decision until the intent is gone. The owner binding
    // remains until this exact directory has actually disappeared.
    files.sort_by_key(|(path, _)| path.file_name().is_some_and(|name| name == COMMITTED));
    for (file, identity) in files {
        remove_name(&file, &identity, false)?;
        if file.file_name().is_some_and(|name| name == INTENT) {
            observe(Point::IntentRetired)?;
        }
    }
    observe(Point::ControlsRetired)?;
    drop(transaction);
    drop(_owner);
    remove_name(&path, &binding.directory_identity, true)?;
    forget_binding(owner, id, &binding.plan.task, completed)
}
