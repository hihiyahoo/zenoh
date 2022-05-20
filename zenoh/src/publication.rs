//
// Copyright (c) 2022 ZettaScale Technology
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ZettaScale Zenoh Team, <zenoh@zettascale.tech>
//

//! Publishing primitives.

use crate::net::transport::Primitives;
use crate::prelude::*;
use crate::subscriber::Reliability;
use crate::Encoding;
use crate::SessionRef;
use zenoh_core::zresult::ZResult;
use zenoh_core::{zread, AsyncResolve, Resolvable, SyncResolve};
use zenoh_protocol::proto::{data_kind, DataInfo, Options};
use zenoh_protocol_core::Channel;

/// The kind of congestion control.
pub use zenoh_protocol_core::CongestionControl;

/// A builder for initializing a [`delete`](crate::Session::delete) operation.
///
/// # Examples
/// ```
/// # async_std::task::block_on(async {
/// use zenoh::prelude::*;
/// use r#async::AsyncResolve;
/// use zenoh::publication::CongestionControl;
///
/// let session = zenoh::open(config::peer()).res().await.unwrap();
/// session
///     .delete("/key/expression")
///     .res()
///     .await
///     .unwrap();
/// # })
/// ```
pub type DeleteBuilder<'a> = PutBuilder<'a>;

/// A builder for initializing a [`put`](crate::Session::put) operation.
///
/// # Examples
/// ```
/// # async_std::task::block_on(async {
/// use zenoh::prelude::*;
/// use r#async::AsyncResolve;
/// use zenoh::publication::CongestionControl;
///
/// let session = zenoh::open(config::peer()).res().await.unwrap();
/// session
///     .put("/key/expression", "value")
///     .encoding(KnownEncoding::TextPlain)
///     .congestion_control(CongestionControl::Block)
///     .res()
///     .await
///     .unwrap();
/// # })
/// ```
#[derive(Debug, Clone)]
pub struct PutBuilder<'a> {
    pub(crate) publisher: Publisher<'a>,
    pub(crate) value: Value,
    pub(crate) kind: SampleKind,
}

impl PutBuilder<'_> {
    /// Change the encoding of the written data.
    #[inline]
    pub fn encoding<IntoEncoding>(mut self, encoding: IntoEncoding) -> Self
    where
        IntoEncoding: Into<Encoding>,
    {
        self.value.encoding = encoding.into();
        self
    }
    /// Change the `congestion_control` to apply when routing the data.
    #[inline]
    pub fn congestion_control(mut self, congestion_control: CongestionControl) -> Self {
        self.publisher = self.publisher.congestion_control(congestion_control);
        self
    }

    /// Change the priority of the written data.
    #[inline]
    pub fn priority(mut self, priority: Priority) -> Self {
        self.publisher = self.publisher.priority(priority);
        self
    }

    /// Enable or disable local routing.
    #[inline]
    pub fn local_routing(mut self, local_routing: bool) -> Self {
        self.publisher = self.publisher.local_routing(local_routing);
        self
    }
    pub fn kind(mut self, kind: SampleKind) -> Self {
        self.kind = kind;
        self
    }
}

impl Resolvable for PutBuilder<'_> {
    type Output = zenoh_core::Result<()>;
}
impl SyncResolve for PutBuilder<'_> {
    #[inline]
    fn res_sync(self) -> Self::Output {
        self.publisher.write(self.kind, self.value)
    }
}
impl AsyncResolve for PutBuilder<'_> {
    type Future = futures::future::Ready<Self::Output>;

    fn res_async(self) -> Self::Future {
        futures::future::ready(self.res_sync())
    }
}

use futures::Sink;
use std::pin::Pin;
use std::task::{Context, Poll};
use zenoh_core::zresult::Error;

/// A publisher that allows to send data through a stream.
///
/// Publishers are automatically undeclared when dropped.
///
/// # Examples
/// ```
/// # async_std::task::block_on(async {
/// use zenoh::prelude::*;
/// use r#async::AsyncResolve;
///
/// let session = zenoh::open(config::peer()).res().await.unwrap().into_arc();
/// let publisher = session.publish("/key/expression").res().await.unwrap();
/// publisher.put("value").unwrap();
/// # })
/// ```
///
///
/// `Publisher` implements the `Sink` trait which is useful to forward
/// streams to zenoh.
/// ```no_run
/// # async_std::task::block_on(async {
/// use zenoh::prelude::*;
/// use r#async::AsyncResolve;
///
/// let session = zenoh::open(config::peer()).res().await.unwrap().into_arc();
/// let mut subscriber = session.subscribe("/key/expression").res().await.unwrap();
/// let publisher = session.publish("/another/key/expression").res().await.unwrap();
/// subscriber.forward(publisher).await.unwrap();
/// # })
/// ```
#[derive(Debug, Clone)]
pub struct Publisher<'a> {
    pub(crate) session: SessionRef<'a>,
    pub(crate) key_expr: KeyExpr<'a>,
    pub(crate) congestion_control: CongestionControl,
    pub(crate) priority: Priority,
    pub(crate) local_routing: Option<bool>,
}

impl Publisher<'_> {
    /// Change the `congestion_control` to apply when routing the data.
    #[inline]
    pub fn congestion_control(mut self, congestion_control: CongestionControl) -> Self {
        self.congestion_control = congestion_control;
        self
    }

    /// Change the priority of the written data.
    #[inline]
    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Enable or disable local routing.
    #[inline]
    pub fn local_routing(mut self, local_routing: bool) -> Self {
        self.local_routing = Some(local_routing);
        self
    }

    pub fn write(&self, kind: SampleKind, value: Value) -> zenoh_core::Result<()> {
        log::trace!("write({:?}, [...])", self.key_expr);
        let state = zread!(self.session.state);
        let primitives = state.primitives.as_ref().unwrap().clone();
        drop(state);

        let mut info = DataInfo::new();
        let kind = kind as u64;
        info.kind = match kind {
            data_kind::DEFAULT => None,
            kind => Some(kind),
        };
        info.encoding = if value.encoding != Encoding::default() {
            Some(value.encoding)
        } else {
            None
        };
        info.timestamp = self.session.runtime.new_timestamp();
        let data_info = if info.has_options() { Some(info) } else { None };

        primitives.send_data(
            &self.key_expr,
            value.payload.clone(),
            Channel {
                priority: self.priority.into(),
                reliability: Reliability::Reliable, // @TODO: need to check subscriptions to determine the right reliability value
            },
            self.congestion_control,
            data_info.clone(),
            None,
        );
        self.session.handle_data(
            true,
            &self.key_expr,
            data_info,
            value.payload,
            self.local_routing,
        );
        Ok(())
    }
    /// Send a value.
    ///
    /// # Examples
    /// ```
    /// # async_std::task::block_on(async {
    /// use zenoh::prelude::*;
    /// use r#async::AsyncResolve;
    ///
    /// let session = zenoh::open(config::peer()).res().await.unwrap().into_arc();
    /// let publisher = session.publish("/key/expression").res().await.unwrap();
    /// publisher.put("value").unwrap();
    /// # })
    /// ```
    #[inline]
    pub fn put<IntoValue>(&self, value: IntoValue) -> zenoh_core::Result<()>
    where
        IntoValue: Into<Value>,
    {
        self.write(SampleKind::Put, value.into())
    }
    pub fn delete(&self) -> zenoh_core::Result<()> {
        self.write(SampleKind::Delete, Value::empty())
    }
}

impl<'a, IntoValue> Sink<IntoValue> for Publisher<'a>
where
    IntoValue: Into<Value>,
{
    type Error = Error;

    #[inline]
    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    #[inline]
    fn start_send(self: Pin<&mut Self>, item: IntoValue) -> Result<(), Self::Error> {
        self.put(item.into())
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    #[inline]
    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// A builder for initializing a [`Publisher`](Publisher).
///
/// # Examples
/// ```
/// # async_std::task::block_on(async {
/// use zenoh::prelude::*;
/// use r#async::AsyncResolve;
/// use zenoh::publication::CongestionControl;
///
/// let session = zenoh::open(config::peer()).res().await.unwrap();
/// let publisher = session
///     .publish("/key/expression")
///     .congestion_control(CongestionControl::Block)
///     .res()
///     .await
///     .unwrap();
/// # })
/// ```
#[derive(Debug, Clone)]
pub struct PublishBuilder<'a> {
    pub(crate) publisher: Publisher<'a>,
}

impl<'a> PublishBuilder<'a> {
    /// Change the `congestion_control` to apply when routing the data.
    #[inline]
    pub fn congestion_control(mut self, congestion_control: CongestionControl) -> Self {
        self.publisher = self.publisher.congestion_control(congestion_control);
        self
    }

    /// Change the priority of the written data.
    #[inline]
    pub fn priority(mut self, priority: Priority) -> Self {
        self.publisher = self.publisher.priority(priority);
        self
    }

    /// Enable or disable local routing.
    #[inline]
    pub fn local_routing(mut self, local_routing: bool) -> Self {
        self.publisher = self.publisher.local_routing(local_routing);
        self
    }
}

impl<'a> Resolvable for PublishBuilder<'a> {
    type Output = ZResult<Publisher<'a>>;
}
impl SyncResolve for PublishBuilder<'_> {
    #[inline]
    fn res_sync(self) -> Self::Output {
        let publisher = self.publisher;
        log::trace!("publish({:?})", publisher.key_expr);
        Ok(publisher)
    }
}
impl AsyncResolve for PublishBuilder<'_> {
    type Future = futures::future::Ready<Self::Output>;

    fn res_async(self) -> Self::Future {
        futures::future::ready(self.res_sync())
    }
}
