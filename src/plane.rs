// Copyright (c) 2017-2018, The rav1e contributors. All rights reserved
//
// This source code is subject to the terms of the BSD 2 Clause License and
// the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
// was not distributed with this source code in the LICENSE file, you can
// obtain it at www.aomedia.org/license/software. If the Alliance for Open
// Media Patent License 1.0 was not distributed with this source code in the
// PATENTS file, you can obtain it at www.aomedia.org/license/patent.

use std::iter::FusedIterator;
use std::fmt::{Debug, Display, Formatter};
use std::mem;

use crate::util::*;

/// Plane-specific configuration.
#[derive(Debug, Clone)]
pub struct PlaneConfig {
  pub stride: usize,
  pub alloc_height: usize,
  pub width: usize,
  pub height: usize,
  pub xdec: usize,
  pub ydec: usize,
  pub xpad: usize,
  pub ypad: usize,
  pub xorigin: usize,
  pub yorigin: usize
}

/// Absolute offset in pixels inside a plane
#[derive(Debug)]
pub struct PlaneOffset {
  pub x: isize,
  pub y: isize
}

#[derive(Clone)]
pub struct Plane<T: Pixel> {
  pub data: Vec<T>,
  pub cfg: PlaneConfig
}

impl<T: Pixel> Debug for Plane<T>
    where T: Display {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    write!(f, "Plane {{ data: [{}, ...], cfg: {:?} }}", self.data[0], self.cfg)
  }
}

impl<T: Pixel> Plane<T> {
  /// Stride alignment in bytes.
  const STRIDE_ALIGNMENT_LOG2: usize = 4;

  /// Data alignment in bytes.
  const DATA_ALIGNMENT_LOG2: usize = 4;

  pub fn new(
    width: usize, height: usize, xdec: usize, ydec: usize, xpad: usize,
    ypad: usize
  ) -> Self {
    let xorigin = xpad.align_power_of_two(Self::STRIDE_ALIGNMENT_LOG2 - 1);
    let yorigin = ypad;
    let stride = (xorigin + width + xpad)
      .align_power_of_two(Self::STRIDE_ALIGNMENT_LOG2 - 1);
    let alloc_height = yorigin + height + ypad;
    let data = vec![T::cast_from(128); stride * alloc_height];
    assert!(is_aligned(data.as_ptr(), Self::DATA_ALIGNMENT_LOG2));
    Plane {
      data,
      cfg: PlaneConfig {
        stride,
        alloc_height,
        width,
        height,
        xdec,
        ydec,
        xpad,
        ypad,
        xorigin,
        yorigin
      }
    }
  }

  pub fn pad(&mut self, w: usize, h: usize) {
    let xorigin = self.cfg.xorigin;
    let yorigin = self.cfg.yorigin;
    let stride = self.cfg.stride;
    let width = w >> self.cfg.xdec;
    let height = h >> self.cfg.ydec;

    if xorigin > 0 {
      for y in 0..height {
        let mut ps = self
          .mut_slice(&PlaneOffset { x: -(xorigin as isize), y: y as isize });
        let s = ps.as_mut_slice_w_width(xorigin + 1);
        let fill_val = s[xorigin];
        for val in s[..xorigin].iter_mut() {
          *val = fill_val;
        }
      }
    }

    if xorigin + width < stride {
      for y in 0..height {
        let mut ps = self
          .mut_slice(&PlaneOffset { x: width as isize - 1, y: y as isize });
        let s = ps.as_mut_slice_w_width(stride - xorigin - width + 1);
        let fill_val = s[0];
        for val in s[1..].iter_mut() {
          *val = fill_val;
        }
      }
    }

    if yorigin > 0 {
      let mut ps = self.mut_slice(&PlaneOffset {
        x: -(xorigin as isize),
        y: -(yorigin as isize)
      });
      let (s1, s2) = ps.as_mut_slice().split_at_mut(yorigin * stride);
      for y in 0..yorigin {
        s1[y * stride..y * stride + stride].copy_from_slice(&s2[..stride]);
      }
    }

    if yorigin + height < self.cfg.alloc_height {
      let mut ps = self.mut_slice(&PlaneOffset {
        x: -(xorigin as isize),
        y: height as isize - 1
      });
      let (s2, s1) = ps.as_mut_slice().split_at_mut(stride);
      for y in 0..yorigin {
        s1[y * stride..y * stride + stride].copy_from_slice(&s2[..stride]);
      }
    }
  }

  pub fn slice(&self, po: &PlaneOffset) -> PlaneSlice<'_, T> {
    PlaneSlice { plane: self, x: po.x, y: po.y }
  }

  pub fn mut_slice(&mut self, po: &PlaneOffset) -> PlaneMutSlice<'_, T> {
    PlaneMutSlice { plane: self, x: po.x, y: po.y }
  }

  #[inline]
  fn index(&self, x: usize, y: usize) -> usize {
    (y + self.cfg.yorigin) * self.cfg.stride + (x + self.cfg.xorigin)
  }

  pub fn p(&self, x: usize, y: usize) -> T {
    self.data[self.index(x, y)]
  }

  pub fn data_origin(&self) -> &[T] {
    &self.data[self.index(0, 0)..]
  }

  pub fn data_origin_mut(&mut self) -> &mut [T] {
    let i = self.index(0, 0);
    &mut self.data[i..]
  }

  pub fn copy_from_raw_u8(
    &mut self, source: &[u8], source_stride: usize, source_bytewidth: usize
  ) {
    let stride = self.cfg.stride;
    for (self_row, source_row) in self
      .data_origin_mut()
      .chunks_mut(stride)
      .zip(source.chunks(source_stride))
    {
      match source_bytewidth {
        1 => for (self_pixel, source_pixel) in
          self_row.iter_mut().zip(source_row.iter())
        {
          *self_pixel = T::cast_from(*source_pixel);
        },
        2 => {
          assert!(mem::size_of::<T>() >= 2, "source bytewidth ({}) cannot fit in Plane<u8>", source_bytewidth);
          for (self_pixel, bytes) in
            self_row.iter_mut().zip(source_row.chunks(2))
          {
            *self_pixel = T::cast_from(u16::cast_from(bytes[1]) << 8 | u16::cast_from(bytes[0]));
          }
        },

        _ => {}
      }
    }
  }

  pub fn downsample_from(&mut self, src: &Plane<T>) {
    let width = self.cfg.width;
    let height = self.cfg.height;

    assert!(width * 2 == src.cfg.width);
    assert!(height * 2 == src.cfg.height);

    for row in 0..height {
      let mut dst_slice = self.mut_slice(&PlaneOffset{ x: 0, y: row as isize });
      let dst = dst_slice.as_mut_slice();

      for col in 0..width {
        let mut sum = 0;
        sum += u32::cast_from(src.p(2 * col, 2 * row));
        sum += u32::cast_from(src.p(2 * col + 1, 2 * row));
        sum += u32::cast_from(src.p(2 * col, 2 * row + 1));
        sum += u32::cast_from(src.p(2 * col + 1, 2 * row + 1));
        let avg = (sum + 2) >> 2;
        dst[col] = T::cast_from(avg);
      }
    }
  }

  /// Iterates over the pixels in the `Plane`, skipping stride data.
  pub fn iter(&self) -> PlaneIter<'_, T> {
    PlaneIter::new(self)
  }
}

#[derive(Debug)]
pub struct PlaneIter<'a, T: Pixel> {
  plane: &'a Plane<T>,
  y: usize,
  x: usize,
}

impl<'a, T: Pixel> PlaneIter<'a, T> {
  pub fn new(plane: &'a Plane<T>) -> Self {
    Self {
      plane,
      y: 0,
      x: 0,
    }
  }

  fn width(&self) -> usize {
    self.plane.cfg.width
  }

  fn height(&self) -> usize {
    self.plane.cfg.height
  }
}

impl<'a, T: Pixel> Iterator for PlaneIter<'a, T> {
  type Item = T;

  fn next(&mut self) -> Option<<Self as Iterator>::Item> {
    if self.y == self.height() {
      return None;
    }
    let pixel = self.plane.p(self.x, self.y);
    if self.x == self.width() - 1 {
      self.x = 0;
      self.y += 1;
    } else {
      self.x += 1;
    }
    Some(pixel)
  }
}

#[derive(Clone, Copy, Debug)]
pub struct PlaneSlice<'a, T: Pixel> {
  pub plane: &'a Plane<T>,
  pub x: isize,
  pub y: isize
}

pub struct IterWidth<'a, T: Pixel> {
    ps: PlaneSlice<'a, T>,
    width: usize,
}

impl<'a, T: Pixel> Iterator for IterWidth<'a, T> {
    type Item = &'a [T];

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let x = self.ps.plane.cfg.xorigin as isize + self.ps.x;
        let y = self.ps.plane.cfg.yorigin as isize + self.ps.y;
        let stride = self.ps.plane.cfg.stride;
        let base = y as usize * stride + x as usize;

        if self.ps.plane.data.len() < base + self.width {
            None
        } else {
            self.ps.y += 1;
            Some(&self.ps.plane.data[base..base + self.width])
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let size = self.ps.plane.cfg.height - self.ps.y as usize;

        (size, Some(size))
    }
}

impl<'a, T: Pixel> ExactSizeIterator for IterWidth<'a, T> { }

impl<'a, T: Pixel> FusedIterator for IterWidth<'a, T> { }

impl<'a, T: Pixel> PlaneSlice<'a, T> {
  pub fn as_slice(&self) -> &'a [T] {
    let stride = self.plane.cfg.stride;
    let base = (self.y + self.plane.cfg.yorigin as isize) as usize * stride
      + (self.x + self.plane.cfg.xorigin as isize) as usize;
    &self.plane.data[base..]
  }

  pub fn as_slice_clamped(&self) -> &'a [T] {
    let stride = self.plane.cfg.stride;
    let y = (self.y.min(self.plane.cfg.height as isize)
      + self.plane.cfg.yorigin as isize)
      .max(0) as usize;
    let x = (self.x.min(self.plane.cfg.width as isize)
      + self.plane.cfg.xorigin as isize)
      .max(0) as usize;
    &self.plane.data[y * stride + x..]
  }

  pub fn clamp(&self) -> PlaneSlice<'a, T> {
    PlaneSlice {
      plane: self.plane,
      x: self
        .x
        .min(self.plane.cfg.width as isize)
        .max(-(self.plane.cfg.xorigin as isize)),
      y: self
        .y
        .min(self.plane.cfg.height as isize)
        .max(-(self.plane.cfg.yorigin as isize))
    }
  }

  pub fn as_slice_w_width(&self, width: usize) -> &'a [T] {
    let stride = self.plane.cfg.stride;
    let base = (self.y + self.plane.cfg.yorigin as isize) as usize * stride
      + (self.x + self.plane.cfg.xorigin as isize) as usize;
    &self.plane.data[base..base + width]
  }

  pub fn iter_width(&self, width: usize) -> IterWidth<'a, T> {
    IterWidth { ps: *self, width }
  }

  pub fn subslice(&self, xo: usize, yo: usize) -> PlaneSlice<'a, T> {
    PlaneSlice {
      plane: self.plane,
      x: self.x + xo as isize,
      y: self.y + yo as isize
    }
  }

  pub fn reslice(&self, xo: isize, yo: isize) -> PlaneSlice<'a, T> {
    PlaneSlice {
      plane: self.plane,
      x: self.x + xo,
      y: self.y + yo
    }
  }

  /// A slice starting i pixels above the current one.
  pub fn go_up(&self, i: usize) -> PlaneSlice<'a, T> {
    PlaneSlice { plane: self.plane, x: self.x, y: self.y - i as isize }
  }

  /// A slice starting i pixels to the left of the current one.
  pub fn go_left(&self, i: usize) -> PlaneSlice<'a, T> {
    PlaneSlice { plane: self.plane, x: self.x - i as isize, y: self.y }
  }

  pub fn p(&self, add_x: usize, add_y: usize) -> T {
    let new_y =
      (self.y + add_y as isize + self.plane.cfg.yorigin as isize) as usize;
    let new_x =
      (self.x + add_x as isize + self.plane.cfg.xorigin as isize) as usize;
    self.plane.data[new_y * self.plane.cfg.stride + new_x]
  }
}

pub struct PlaneMutSlice<'a, T: Pixel> {
  pub plane: &'a mut Plane<T>,
  pub x: isize,
  pub y: isize
}

impl<'a, T: Pixel> PlaneMutSlice<'a, T> {
  pub fn as_mut_slice(&mut self) -> &mut [T] {
    let stride = self.plane.cfg.stride;
    let base = (self.y + self.plane.cfg.yorigin as isize) as usize * stride
      + (self.x + self.plane.cfg.xorigin as isize) as usize;
    &mut self.plane.data[base..]
  }

  pub fn as_mut_slice_w_width(&mut self, width: usize) -> &mut [T] {
    let stride = self.plane.cfg.stride;
    let y = self.y + self.plane.cfg.yorigin as isize;
    let x = self.x + self.plane.cfg.xorigin as isize;
    assert!(y >= 0);
    assert!(x >= 0);
    let base = y as usize * stride + x as usize;
    &mut self.plane.data[base..base + width]
  }

  pub fn offset(&self, add_x: usize, add_y: usize) -> &[T] {
    let new_y =
      (self.y + add_y as isize + self.plane.cfg.yorigin as isize) as usize;
    let new_x =
      (self.x + add_x as isize + self.plane.cfg.xorigin as isize) as usize;
    &self.plane.data[new_y * self.plane.cfg.stride + new_x..]
  }

  pub fn offset_as_mutable(
    &mut self, add_x: usize, add_y: usize
  ) -> &mut [T] {
    let new_y =
      (self.y + add_y as isize + self.plane.cfg.yorigin as isize) as usize;
    let new_x =
      (self.x + add_x as isize + self.plane.cfg.xorigin as isize) as usize;
    &mut self.plane.data[new_y * self.plane.cfg.stride + new_x..]
  }

  // FIXME: code duplication with PlaneSlice

  /// A slice starting i pixels above the current one.
  pub fn go_up(&self, i: usize) -> PlaneSlice<'_, T> {
    PlaneSlice { plane: self.plane, x: self.x, y: self.y - i as isize }
  }

  /// A slice starting i pixels to the left of the current one.
  pub fn go_left(&self, i: usize) -> PlaneSlice<'_, T> {
    PlaneSlice { plane: self.plane, x: self.x - i as isize, y: self.y }
  }

  pub fn p(&self, add_x: usize, add_y: usize) -> T {
    let new_y =
      (self.y + add_y as isize + self.plane.cfg.yorigin as isize) as usize;
    let new_x =
      (self.x + add_x as isize + self.plane.cfg.xorigin as isize) as usize;
    self.plane.data[new_y * self.plane.cfg.stride + new_x]
  }
}
