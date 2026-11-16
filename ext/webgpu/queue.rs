// Copyright 2018-2026 the Deno authors. MIT license.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use deno_core::GarbageCollected;
use deno_core::WebIDL;
use deno_core::cppgc::Ref;
use deno_core::futures::channel::oneshot;
use deno_core::op2;
use deno_core::v8;
use deno_error::JsErrorBox;

use crate::Instance;
use crate::buffer::GPUBuffer;
use crate::command_buffer::GPUCommandBuffer;
use crate::error::GPUGenericError;
use crate::texture::GPUTexture;
use crate::texture::GPUTextureAspect;
use crate::webidl::GPUExtent3D;
use crate::webidl::GPUOrigin3D;

pub struct GPUQueue {
  pub instance: Instance,
  pub error_handler: super::error::ErrorHandler,

  pub label: String,

  pub id: wgpu_core::id::QueueId,
  pub device: wgpu_core::id::DeviceId,
}

impl Drop for GPUQueue {
  fn drop(&mut self) {
    self.instance.queue_drop(self.id);
  }
}

// SAFETY: we're sure this can be GCed
unsafe impl GarbageCollected for GPUQueue {
  fn trace(&self, _visitor: &mut deno_core::v8::cppgc::Visitor) {}
  fn get_name(&self) -> &'static std::ffi::CStr {
    c"GPUQueue"
  }
}

#[op2]
impl GPUQueue {
  #[constructor]
  #[cppgc]
  fn constructor(_: bool) -> Result<GPUQueue, GPUGenericError> {
    Err(GPUGenericError::InvalidConstructor)
  }

  #[getter]
  #[string]
  fn label(&self) -> String {
    self.label.clone()
  }
  #[setter]
  #[string]
  fn label(&self, #[webidl] _label: String) {
    // TODO(@crowlKats): no-op, needs wpgu to implement changing the label
  }

  #[required(1)]
  #[undefined]
  fn submit(
    &self,
    #[webidl] command_buffers: Vec<Ref<GPUCommandBuffer>>,
  ) -> Result<(), JsErrorBox> {
    let ids = command_buffers
      .into_iter()
      .map(|cb| cb.id)
      .collect::<Vec<_>>();

    let err = self.instance.queue_submit(self.id, &ids).err();

    if let Some((_, err)) = err {
      self.error_handler.push_error(Some(err));
    }

    Ok(())
  }

  // In the successful case, the promise should resolve to undefined, but
  // `#[undefined]` does not seem to work here.
  // https://github.com/denoland/deno/issues/29603
  async fn on_submitted_work_done(&self) -> Result<(), JsErrorBox> {
    let (sender, receiver) = oneshot::channel::<()>();

    let callback = Box::new(move || {
      sender.send(()).unwrap();
    });

    self
      .instance
      .queue_on_submitted_work_done(self.id, callback);

    let done = Rc::new(RefCell::new(false));
    let done_ = done.clone();
    let device_poll_fut = async move {
      while !*done.borrow() {
        {
          self
            .instance
            .device_poll(self.device, wgpu_types::PollType::wait_indefinitely())
            .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
      }
      Ok::<(), JsErrorBox>(())
    };

    let receiver_fut = async move {
      receiver
        .await
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;
      let mut done = done_.borrow_mut();
      *done = true;
      Ok::<(), JsErrorBox>(())
    };

    tokio::try_join!(device_poll_fut, receiver_fut)?;

    Ok(())
  }

  #[required(3)]
  #[undefined]
  fn write_buffer<'a>(
    &self,
    scope: &mut v8::PinScope<'a, '_>,
    #[webidl] buffer: Ref<GPUBuffer>,
    #[webidl(options(enforce_range = true))] buffer_offset: u64,
    data_arg: v8::Local<'a, v8::Value>,
    #[webidl(default = 0, options(enforce_range = true))] data_offset: u64,
    #[webidl(options(enforce_range = true))] size: Option<u64>,
  ) -> Result<(), JsErrorBox> {
    // Per the WebGPU spec, dataOffset and size are in elements (not bytes)
    // when data is a TypedArray, and in bytes otherwise.
    let (array_buffer, byte_offset, byte_len, bytes_per_element) = if let Ok(
      typed_array,
    ) =
      v8::Local::<v8::TypedArray>::try_from(data_arg)
    {
      let len = typed_array.length();
      let bpe = typed_array.byte_length().checked_div(len).unwrap_or(1);
      let byte_offset = typed_array.byte_offset();
      let byte_len = typed_array.byte_length();
      let array_buffer = typed_array
        .buffer(scope)
        .ok_or_else(|| JsErrorBox::type_error("data buffer is detached"))?;
      (array_buffer, byte_offset, byte_len, bpe)
    } else if let Ok(array_buffer) =
      v8::Local::<v8::ArrayBuffer>::try_from(data_arg)
    {
      let byte_len = array_buffer.byte_length();
      (array_buffer, 0, byte_len, 1)
    } else if let Ok(view) =
      v8::Local::<v8::ArrayBufferView>::try_from(data_arg)
    {
      let byte_offset = view.byte_offset();
      let byte_len = view.byte_length();
      let array_buffer = view
        .buffer(scope)
        .ok_or_else(|| JsErrorBox::type_error("data buffer is detached"))?;
      (array_buffer, byte_offset, byte_len, 1)
    } else {
      return Err(JsErrorBox::type_error(
        "data must be an ArrayBuffer or ArrayBufferView",
      ));
    };

    if array_buffer.was_detached() {
      return Err(JsErrorBox::type_error("data buffer is detached"));
    }

    // A zero-length ArrayBuffer has no backing store pointer. Constructing a
    // zero-length slice from a null pointer is invalid even when never read.
    let buf: &[u8] = if byte_len == 0 {
      &[]
    } else {
      let ptr = array_buffer
        .data()
        .ok_or_else(|| JsErrorBox::type_error("data buffer is detached"))?;
      // SAFETY: V8 guarantees the view's byte_offset and byte_len are within
      // its backing store, which remains alive for this synchronous operation.
      unsafe {
        std::slice::from_raw_parts(
          ptr.as_ptr().cast::<u8>().add(byte_offset),
          byte_len,
        )
      }
    };

    let data_offset_bytes = data_offset
      .checked_mul(bytes_per_element as u64)
      .and_then(|n| usize::try_from(n).ok())
      .ok_or_else(|| {
        JsErrorBox::new("DOMExceptionOperationError", "dataOffset is too large")
      })?;
    let end = if let Some(size) = size {
      let size_bytes = size
        .checked_mul(bytes_per_element as u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| {
          JsErrorBox::new("DOMExceptionOperationError", "size is too large")
        })?;
      data_offset_bytes.checked_add(size_bytes).ok_or_else(|| {
        JsErrorBox::new(
          "DOMExceptionOperationError",
          "dataOffset and size overflow",
        )
      })?
    } else {
      buf.len()
    };
    let data = buf.get(data_offset_bytes..end).ok_or_else(|| {
      JsErrorBox::new(
        "DOMExceptionOperationError",
        "dataOffset and size exceed data length",
      )
    })?;

    let err = self
      .instance
      .queue_write_buffer(self.id, buffer.id, buffer_offset, data)
      .err();

    self.error_handler.push_error(err);

    Ok(())
  }

  #[required(4)]
  #[undefined]
  fn write_texture(
    &self,
    #[webidl] destination: GPUTexelCopyTextureInfo,
    #[anybuffer] buf: &[u8],
    #[webidl] data_layout: GPUTexelCopyBufferLayout,
    #[webidl] size: GPUExtent3D,
  ) {
    let destination = wgpu_types::TexelCopyTextureInfo {
      texture: destination.texture.id,
      mip_level: destination.mip_level,
      origin: destination.origin.into(),
      aspect: destination.aspect.into(),
    };

    let data_layout = wgpu_types::TexelCopyBufferLayout {
      offset: data_layout.offset,
      bytes_per_row: data_layout.bytes_per_row,
      rows_per_image: data_layout.rows_per_image,
    };

    let err = self
      .instance
      .queue_write_texture(
        self.id,
        &destination,
        buf,
        &data_layout,
        &size.into(),
      )
      .err();

    self.error_handler.push_error(err);
  }
}

#[derive(WebIDL)]
#[webidl(dictionary)]
pub(crate) struct GPUTexelCopyTextureInfo {
  pub texture: Ref<GPUTexture>,
  #[webidl(default = 0)]
  #[options(enforce_range = true)]
  pub mip_level: u32,
  #[webidl(default = Default::default())]
  pub origin: GPUOrigin3D,
  #[webidl(default = GPUTextureAspect::All)]
  pub aspect: GPUTextureAspect,
}

#[derive(WebIDL)]
#[webidl(dictionary)]
struct GPUTexelCopyBufferLayout {
  #[webidl(default = 0)]
  #[options(enforce_range = true)]
  offset: u64,
  #[options(enforce_range = true)]
  bytes_per_row: Option<u32>,
  #[options(enforce_range = true)]
  rows_per_image: Option<u32>,
}
