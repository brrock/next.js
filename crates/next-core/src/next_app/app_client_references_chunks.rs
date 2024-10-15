use anyhow::Result;
use tracing::Instrument;
use turbo_tasks::{
    FxIndexMap, RcStr, TryFlatJoinIterExt, TryJoinIterExt, Value, ValueToString, Vc,
};
use turbopack_core::{
    chunk::{availability_info::AvailabilityInfo, ChunkingContext, ChunkingContextExt},
    module::{Module, Modules},
    output::OutputAssets,
};

use super::include_modules_module::IncludeModulesModule;
use crate::{
    next_client_reference::{
        visit_client_reference::ClientReferenceGraphResult, ClientReferenceType,
        ClientReferenceTypes,
    },
    next_server_component::server_component_module::NextServerComponentModule,
};

#[turbo_tasks::function]
pub fn client_modules_modifier() -> Vc<RcStr> {
    Vc::cell("client modules".into())
}

#[turbo_tasks::function]
pub fn client_modules_ssr_modifier() -> Vc<RcStr> {
    Vc::cell("client modules ssr".into())
}

#[turbo_tasks::function]
pub fn client_modules_rsc_modifier() -> Vc<RcStr> {
    Vc::cell("client modules rsc".into())
}

#[turbo_tasks::value]
pub struct ClientReferencesChunks {
    pub client_component_client_chunks:
        FxIndexMap<ClientReferenceType, (Vc<OutputAssets>, AvailabilityInfo)>,
    pub client_component_ssr_chunks:
        FxIndexMap<ClientReferenceType, (Vc<OutputAssets>, AvailabilityInfo)>,
    pub client_component_rsc_chunks:
        FxIndexMap<ClientReferenceType, (Vc<OutputAssets>, AvailabilityInfo)>,
    pub layout_segment_client_chunks: FxIndexMap<Vc<NextServerComponentModule>, Vc<OutputAssets>>,
}

/// Computes all client references chunks.
///
/// This returns a map from client reference type to the chunks that reference
/// type needs to load.
#[turbo_tasks::function]
pub async fn get_app_client_references_chunks(
    app_client_references: Vc<ClientReferenceGraphResult>,
    client_chunking_context: Vc<Box<dyn ChunkingContext>>,
    client_availability_info: Value<AvailabilityInfo>,
    ssr_chunking_context: Option<Vc<Box<dyn ChunkingContext>>>,
) -> Result<Vc<ClientReferencesChunks>> {
    async move {
        // TODO Reconsider this. Maybe it need to be true in production.
        let separate_chunk_group_per_client_reference = false;
        let app_client_references = app_client_references.await?;
        if separate_chunk_group_per_client_reference {
            let app_client_references_chunks: Vec<(_, (_, Option<_>, Option<_>))> =
                app_client_references
                    .client_references
                    .iter()
                    .map(|client_reference| async move {
                        let client_reference_ty = client_reference.ty();
                        Ok((
                            client_reference_ty,
                            match client_reference_ty {
                                ClientReferenceType::EcmascriptClientReference {
                                    parent_module,
                                    module: ecmascript_client_reference,
                                    ..
                                } => {
                                    let ecmascript_client_reference_ref =
                                        ecmascript_client_reference.await?;

                                    let client_chunk_group = client_chunking_context
                                        .root_chunk_group(Vc::upcast(
                                            ecmascript_client_reference_ref.client_module,
                                        ))
                                        .await?;

                                    (
                                        (
                                            client_chunk_group.assets,
                                            client_chunk_group.availability_info,
                                        ),
                                        if let Some(ssr_chunking_context) = ssr_chunking_context {
                                            let ssr_chunk_group = ssr_chunking_context
                                                .root_chunk_group(Vc::upcast(
                                                    ecmascript_client_reference_ref.ssr_module,
                                                ))
                                                .await?;

                                            Some((
                                                ssr_chunk_group.assets,
                                                ssr_chunk_group.availability_info,
                                            ))
                                        } else {
                                            None
                                        },
                                        if let Some(ssr_chunking_context) = ssr_chunking_context {
                                            let ssr_chunk_group = ssr_chunking_context
                                                .root_chunk_group(Vc::upcast(parent_module))
                                                .await?;

                                            Some((
                                                ssr_chunk_group.assets,
                                                ssr_chunk_group.availability_info,
                                            ))
                                        } else {
                                            None
                                        },
                                    )
                                }
                                ClientReferenceType::CssClientReference(css_module) => {
                                    let client_chunk_group = client_chunking_context
                                        .root_chunk_group(Vc::upcast(css_module))
                                        .await?;

                                    (
                                        (
                                            client_chunk_group.assets,
                                            client_chunk_group.availability_info,
                                        ),
                                        None,
                                        None,
                                    )
                                }
                            },
                        ))
                    })
                    .try_join()
                    .await?;

            Ok(ClientReferencesChunks {
                client_component_client_chunks: app_client_references_chunks
                    .iter()
                    .map(|&(client_reference_ty, (client_chunks, _, _))| {
                        (client_reference_ty, client_chunks)
                    })
                    .collect(),
                client_component_ssr_chunks: app_client_references_chunks
                    .iter()
                    .flat_map(|&(client_reference_ty, (_, ssr_chunks, _))| {
                        ssr_chunks.map(|ssr_chunks| (client_reference_ty, ssr_chunks))
                    })
                    .collect(),
                client_component_rsc_chunks: app_client_references_chunks
                    .iter()
                    .flat_map(|&(client_reference_ty, (_, _, rsc_chunks))| {
                        rsc_chunks.map(|rsc_chunks| (client_reference_ty, rsc_chunks))
                    })
                    .collect(),
                layout_segment_client_chunks: FxIndexMap::default(),
            }
            .cell())
        } else {
            let mut client_references_by_server_component: FxIndexMap<_, Vec<_>> =
                FxIndexMap::default();
            let mut framework_reference_types = Vec::new();
            for &server_component in app_client_references.server_component_entries.iter() {
                client_references_by_server_component
                    .entry(server_component)
                    .or_default();
            }
            for client_reference in app_client_references.client_references.iter() {
                if let Some(server_component) = client_reference.server_component() {
                    client_references_by_server_component
                        .entry(server_component)
                        .or_default()
                        .push(client_reference.ty());
                } else {
                    framework_reference_types.push(client_reference.ty());
                }
            }
            // Framework components need to go into first layout segment
            if let Some((_, list)) = client_references_by_server_component.first_mut() {
                list.extend(framework_reference_types);
            }

            let mut current_client_availability_info = client_availability_info.into_value();
            let mut current_client_chunks = OutputAssets::empty();
            let mut current_ssr_availability_info = AvailabilityInfo::Root;
            let mut current_ssr_chunks = OutputAssets::empty();
            let mut current_rsc_availability_info = AvailabilityInfo::Root;
            let mut current_rsc_chunks = OutputAssets::empty();

            let mut layout_segment_client_chunks = FxIndexMap::default();
            let mut client_component_ssr_chunks = FxIndexMap::default();
            let mut client_component_rsc_chunks = FxIndexMap::default();
            let mut client_component_client_chunks = FxIndexMap::default();

            for (server_component, client_reference_types) in
                client_references_by_server_component.into_iter()
            {
                let base_ident = server_component.ident();

                let server_path = server_component.server_path();
                let is_layout = server_path.file_stem().await?.as_deref() == Some("layout");
                let server_component_path = server_path.to_string().await?;

                let ssr_modules = client_reference_types
                    .iter()
                    .map(|client_reference_ty| async move {
                        Ok(match client_reference_ty {
                            ClientReferenceType::EcmascriptClientReference {
                                module: ecmascript_client_reference,
                                ..
                            } => {
                                let ecmascript_client_reference_ref =
                                    ecmascript_client_reference.await?;

                                Some(Vc::upcast(ecmascript_client_reference_ref.ssr_module))
                            }
                            _ => None,
                        })
                    })
                    .try_flat_join()
                    .await?;

                let rsc_modules = client_reference_types
                    .iter()
                    .map(|client_reference_ty| async move {
                        Ok(match client_reference_ty {
                            ClientReferenceType::EcmascriptClientReference {
                                parent_module,
                                ..
                            } => Some(Vc::upcast(*parent_module)),
                            _ => None,
                        })
                    })
                    .try_flat_join()
                    .await?;

                let ssr_chunk_group = if !ssr_modules.is_empty() {
                    ssr_chunking_context.map(|ssr_chunking_context| {
                        let _span = tracing::info_span!(
                            "server side rendering",
                            layout_segment = display(&server_component_path),
                        )
                        .entered();

                        let ssr_entry_module = IncludeModulesModule::new(
                            base_ident.with_modifier(client_modules_ssr_modifier()),
                            ssr_modules,
                        );
                        ssr_chunking_context.chunk_group(
                            ssr_entry_module.ident(),
                            Vc::upcast(ssr_entry_module),
                            Value::new(current_ssr_availability_info),
                        )
                    })
                } else {
                    None
                };

                let rsc_chunk_group = if !rsc_modules.is_empty() {
                    ssr_chunking_context.map(|ssr_chunking_context| {
                        let _span = tracing::info_span!(
                            "server side rendering",
                            layout_segment = display(&server_component_path),
                        )
                        .entered();

                        let rsc_entry_module = IncludeModulesModule::new(
                            base_ident.with_modifier(client_modules_rsc_modifier()),
                            rsc_modules,
                        );
                        ssr_chunking_context.chunk_group(
                            rsc_entry_module.ident(),
                            Vc::upcast(rsc_entry_module),
                            Value::new(current_rsc_availability_info),
                        )
                    })
                } else {
                    None
                };

                let client_modules = client_reference_types
                    .iter()
                    .map(|client_reference_ty| async move {
                        Ok(match client_reference_ty {
                            ClientReferenceType::EcmascriptClientReference {
                                module: ecmascript_client_reference,
                                ..
                            } => {
                                let ecmascript_client_reference_ref =
                                    ecmascript_client_reference.await?;
                                Vc::upcast(ecmascript_client_reference_ref.client_module)
                            }
                            ClientReferenceType::CssClientReference(css_module) => {
                                Vc::upcast(*css_module)
                            }
                        })
                    })
                    .try_join()
                    .await?;
                let client_chunk_group = if !client_modules.is_empty() {
                    let _span = tracing::info_span!(
                        "client side rendering",
                        layout_segment = display(&server_component_path),
                    )
                    .entered();

                    let client_entry_module = IncludeModulesModule::new(
                        base_ident.with_modifier(client_modules_modifier()),
                        client_modules,
                    );
                    Some(client_chunking_context.chunk_group(
                        client_entry_module.ident(),
                        Vc::upcast(client_entry_module),
                        Value::new(current_client_availability_info),
                    ))
                } else {
                    None
                };

                if let Some(client_chunk_group) = client_chunk_group {
                    let client_chunk_group = client_chunk_group.await?;

                    let client_chunks =
                        current_client_chunks.concatenate(client_chunk_group.assets);
                    let client_chunks = client_chunks.resolve().await?;

                    if is_layout {
                        current_client_availability_info = client_chunk_group.availability_info;
                        current_client_chunks = client_chunks;
                    }

                    layout_segment_client_chunks.insert(server_component, client_chunks);

                    for &client_reference_ty in client_reference_types.iter() {
                        if let ClientReferenceType::EcmascriptClientReference { .. } =
                            client_reference_ty
                        {
                            client_component_client_chunks.insert(
                                client_reference_ty,
                                (client_chunks, client_chunk_group.availability_info),
                            );
                        }
                    }
                }

                if let Some(ssr_chunk_group) = ssr_chunk_group {
                    let ssr_chunk_group = ssr_chunk_group.await?;

                    let ssr_chunks = current_ssr_chunks.concatenate(ssr_chunk_group.assets);
                    let ssr_chunks = ssr_chunks.resolve().await?;

                    if is_layout {
                        current_ssr_availability_info = ssr_chunk_group.availability_info;
                        current_ssr_chunks = ssr_chunks;
                    }

                    for &client_reference_ty in client_reference_types.iter() {
                        if let ClientReferenceType::EcmascriptClientReference { .. } =
                            client_reference_ty
                        {
                            client_component_ssr_chunks.insert(
                                client_reference_ty,
                                (ssr_chunks, ssr_chunk_group.availability_info),
                            );
                        }
                    }
                }

                if let Some(rsc_chunk_group) = rsc_chunk_group {
                    let rsc_chunk_group = rsc_chunk_group.await?;

                    let rsc_chunks = current_rsc_chunks.concatenate(rsc_chunk_group.assets);
                    let rsc_chunks = rsc_chunks.resolve().await?;

                    if is_layout {
                        current_rsc_availability_info = rsc_chunk_group.availability_info;
                        current_rsc_chunks = rsc_chunks;
                    }

                    for &client_reference_ty in client_reference_types.iter() {
                        if let ClientReferenceType::EcmascriptClientReference { .. } =
                            client_reference_ty
                        {
                            client_component_rsc_chunks.insert(
                                client_reference_ty,
                                (rsc_chunks, rsc_chunk_group.availability_info),
                            );
                        }
                    }
                }
            }

            Ok(ClientReferencesChunks {
                client_component_client_chunks,
                client_component_ssr_chunks,
                client_component_rsc_chunks,
                layout_segment_client_chunks,
            }
            .cell())
        }
    }
    .instrument(tracing::info_span!("process client references"))
    .await
}

/// Crawls all modules emitted in the client transition, returning a list of all
/// client JS modules.
#[turbo_tasks::function]
pub async fn get_app_server_reference_modules(
    app_client_reference_types: Vc<ClientReferenceTypes>,
) -> Result<Vc<Modules>> {
    Ok(Vc::cell(
        app_client_reference_types
            .await?
            .iter()
            .map(|client_reference_ty| async move {
                Ok(match client_reference_ty {
                    ClientReferenceType::EcmascriptClientReference {
                        module: ecmascript_client_reference,
                        ..
                    } => {
                        let ecmascript_client_reference_ref = ecmascript_client_reference.await?;
                        Some(Vc::upcast(ecmascript_client_reference_ref.client_module))
                    }
                    _ => None,
                })
            })
            .try_flat_join()
            .await?,
    ))
}
